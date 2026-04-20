//! Results Channel — mpsc-based results collector for URLs
//!
//! Replaces `Arc<Mutex<Vec<T>>>` with tokio mpsc channel for lock-free,
//! backpressure-protected URL collection in high-concurrency crawlers.
//!
//! # Arquitectura
//!
//! ```text
//! Worker Task 1 ──► channel(256) ──┐
//! Worker Task 2 ──►             ├──► Receiver Worker (tokio::spawn)
//! Worker N ──►                  │         │ owns Vec<DiscoveredUrl>
//!                                     │         ▼
//!                                     │    returns Vec on drop(tx)
//!                                     └────────────────────────────
//! ```
//!
//! # Beneficios
//!
//! - **Zero Lock Contention**: No Mutex in hot path
//! - **Backpressure Natural**: bounded channel + await on send()
//! - **Shutdown Determinista**: El canal se cierra cuando todos los tx mueren

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{debug, error, info, warn};

use crate::domain::DiscoveredUrl;

/// Mensajes para el canal de resultados (URLs descubiertas)
///
/// Usamos DiscoveredUrl porque eso es lo que el crawler colecta.
#[derive(Debug, Clone)]
pub enum CrawlMessage {
    /// URL scrapeada exitosamente
    Success(DiscoveredUrl),
    /// Error durante el scrape
    Error { url: String, error: String },
}

impl CrawlMessage {
    /// Crear mensaje de éxito
    pub fn success(url: DiscoveredUrl) -> Self {
        Self::Success(url)
    }

    /// Crear mensaje de error
    pub fn error(url: impl Into<String>, error: impl Into<String>) -> Self {
        Self::Error {
            url: url.into(),
            error: error.into(),
        }
    }
}

/// Results Collector con canal mpsc para DiscoveredUrl
/// 
/// Esta estructura es DELGADA: solo provee el transmitter y acceso atómico.
/// El worker (tokio::spawn) es el único dueño del Vec de resultados.
/// 
/// # Uso
/// 
/// ```rust
/// let collector = ResultsCollector::new(512, Some(1000));
/// 
/// // En cada worker:
/// collector.send(CrawlMessage::success(url)).await;
///
/// // Al finalizar:
/// let results = collector.collect().await;
/// ```
pub struct ResultsCollector {
    /// Sender para producir mensajes (clonado para cada worker)
    tx: mpsc::Sender<CrawlMessage>,
    /// Contador atómico para verificar max_pages sin lock
    counter: Arc<AtomicUsize>,
    /// Handle del worker para esperar terminación
    handle: Option<JoinHandle<Vec<DiscoveredUrl>>>,
}

impl Clone for ResultsCollector {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            counter: Arc::clone(&self.counter),
            handle: None, // Only original puede collect
        }
    }
}

impl ResultsCollector {
    /// Crear nuevo collector con capacidad especificada
    /// 
    /// # Arguments
    /// 
    /// * `capacity` - Tamaño del buffer del canal (backpressure). 
    /// * `max_capacity` - Pre-allocación para el Vec interno
    pub fn new(capacity: usize, max_capacity: Option<usize>) -> Self {
        let (tx, mut rx) = mpsc::channel(capacity);
        let counter = Arc::new(AtomicUsize::new(0));
        let vec_capacity = max_capacity.unwrap_or(capacity);
        
        // Worker dedicado que posee el receiver y el Vec final
        let _counter_clone = Arc::clone(&counter);
        let handle = tokio::spawn(async move {
            let mut results = Vec::with_capacity(vec_capacity);
            
            // El bucle termina cuando rx se cierra (todos los tx muertos)
            while let Some(msg) = rx.recv().await {
                match msg {
                    CrawlMessage::Success(url) => {
                        debug!("Collected: {}", url.url);
                        results.push(url);
                        // Counter already updated in send()
                    }
                    CrawlMessage::Error { url, error } => {
                        warn!("Error collecting {}: {}", url, error);
                    }
                }
            }
            
            info!("Collector finished: {} URLs", results.len());
            results
        });

        Self {
            tx,
            counter,
            handle: Some(handle),
        }
    }

    /// Versión simple con capacidad por defecto
    pub fn with_capacity(capacity: usize) -> Self {
        Self::new(capacity, None)
    }

    /// Verificar si alcanzamos max_pages (sin lock)
    /// 
    /// Usa AtomicUsize para chequeo O(1) sin bloqueo.
    #[inline]
    pub fn is_full(&self, max_pages: usize) -> bool {
        self.counter.load(Ordering::Relaxed) >= max_pages
    }

    /// Obtener cantidad actual de resultados
    #[inline]
    pub fn len(&self) -> usize {
        self.counter.load(Ordering::Relaxed)
    }

    /// Verificar si el collector está vacío
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Enviar resultado (con backpressure implícito)
    ///
    /// Si el canal está lleno, esta llamada awaitará.
    pub async fn send(&self, msg: CrawlMessage) -> Result<(), mpsc::error::SendError<CrawlMessage>> {
        // Update counter synchronously for is_full() checks
        if let CrawlMessage::Success(_) = &msg {
            self.counter.fetch_add(1, Ordering::Relaxed);
        }
        self.tx.send(msg).await
    }

/// Intentar enviar sin esperar
    /// 
    /// Útil para manejo custom de backpressure.
    /// Retorna error si el canal está lleno.
    pub fn try_send(&self, msg: CrawlMessage) -> Result<(), Box<mpsc::error::TrySendError<CrawlMessage>>> {
        self.tx.try_send(msg).map_err(Box::new)
    }

    /// Recolectar y retornar resultados
    /// 
    /// IMPORANTE: Debe llamarse UNA SOLA VEZ al finalizar el crawl.
    pub async fn collect(mut self) -> Vec<DiscoveredUrl> {
        // Cerrar el canal - el worker recibirá None y terminará
        drop(self.tx);
        
        // Esperar al worker
        if let Some(handle) = self.handle.take() {
            match handle.await {
                Ok(results) => results,
                Err(e) => {
                    error!("Worker panicked: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    }
}

impl Default for ResultsCollector {
    fn default() -> Self {
        Self::new(256, None)
    }
}

/// Adapter para compatibilidad con código existente
/// 
/// Wrapper más simple si solo necesitas enviar URLs.
pub struct ResultsAdapter {
    collector: ResultsCollector,
}

impl ResultsAdapter {
    pub fn new(capacity: usize) -> Self {
        Self {
            collector: ResultsCollector::with_capacity(capacity),
        }
    }

    /// Enviar URL scrapeada exitosamente
    pub async fn add_success(&self, url: DiscoveredUrl) -> Result<(), mpsc::error::SendError<CrawlMessage>> {
        self.collector.send(CrawlMessage::success(url)).await
    }

    /// Enviar error de scrape
    pub async fn add_error(&self, url: String, error: String) -> Result<(), mpsc::error::SendError<CrawlMessage>> {
        self.collector.send(CrawlMessage::error(url, error)).await
    }

    /// Verificar límite
    pub fn is_full(&self, max_pages: usize) -> bool {
        self.collector.is_full(max_pages)
    }

    /// Obtener count
    pub fn len(&self) -> usize {
        self.collector.len()
    }

    /// Verificar si está vacío
    pub fn is_empty(&self) -> bool {
        self.collector.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    fn make_url(url: &str) -> DiscoveredUrl {
        let u = Url::parse(url).unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        DiscoveredUrl::html(u, 0, parent)
    }

    #[tokio::test]
    async fn test_collector_basic() {
        let collector = ResultsCollector::new(100, Some(200));

        collector.send(CrawlMessage::success(make_url("https://a.com"))).await.unwrap();
        collector.send(CrawlMessage::success(make_url("https://b.com"))).await.unwrap();

        assert_eq!(collector.len(), 2);

        let results = collector.collect().await;
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_collector_is_full() {
        let collector = ResultsCollector::new(100, None);

        for i in 0..5 {
            collector.send(CrawlMessage::success(make_url(&format!("https://{}.com", i)))).await.unwrap();
        }

        assert!(collector.is_full(3));
        assert!(!collector.is_full(10));
    }

    #[tokio::test]
    async fn test_collector_concurrent() {
        use tokio::task::JoinSet;

        let collector = ResultsCollector::new(100, None);
        let mut set = JoinSet::new();

        for i in 0..10 {
            let collector = collector.clone();
            set.spawn(async move {
                for j in 0..5 {
                    let url = make_url(&format!("https://worker{}-{}.com", i, j));
                    collector.send(CrawlMessage::success(url)).await.ok();
                }
            });
        }

        while set.join_next().await.is_some() {}

        assert_eq!(collector.len(), 50);

        let results = collector.collect().await;
        assert_eq!(results.len(), 50);
    }
}