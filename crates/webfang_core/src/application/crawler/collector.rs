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
use tracing::{debug, error, info, Instrument};

use crate::domain::DiscoveredUrl;

/// Mensajes para el canal de resultados (URLs descubiertas)
///
/// Usamos DiscoveredUrl porque eso es lo que el crawler colecta.
#[derive(Debug, Clone)]
pub(crate) enum CrawlMessage {
    /// URL scrapeada exitosamente
    Success(DiscoveredUrl),
}

impl CrawlMessage {
    /// Crear mensaje de éxito
    pub fn success(url: DiscoveredUrl) -> Self {
        Self::Success(url)
    }
}

/// Results Collector con canal mpsc para DiscoveredUrl
///
/// Esta estructura es DELGADA: solo provee el transmitter y acceso atómico.
/// El worker (tokio::spawn) es el único dueño del Vec de resultados.
///
/// # Uso
///
/// ```compile_fail
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
        // tokio mpsc panics on a zero buffer (#780); the CLI parser rejects
        // --max-pages 0, but programmatic / config-file paths still need a
        // backstop. Clamp to 1: identical semantics for the crawl (backpressure
        // channel of 1 slot, no silent loss).
        let capacity = capacity.max(1);
        let (tx, mut rx) = mpsc::channel(capacity);
        let counter = Arc::new(AtomicUsize::new(0));
        let vec_capacity = max_capacity.unwrap_or(capacity);

        // Worker dedicado que posee el receiver y el Vec final
        let _counter_clone = Arc::clone(&counter);
        let handle = tokio::spawn(
            async move {
                let mut results = Vec::with_capacity(vec_capacity);

                // El bucle termina cuando rx se cierra (todos los tx muertos)
                while let Some(CrawlMessage::Success(url)) = rx.recv().await {
                    debug!("Collected: {}", url.url);
                    results.push(url);
                    // Counter already updated in send()
                }

                info!("Collector finished: {} URLs", results.len());
                results
            }
            .in_current_span(),
        );

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
    pub(crate) async fn send(
        &self,
        msg: CrawlMessage,
    ) -> Result<(), mpsc::error::SendError<CrawlMessage>> {
        // Every message on this channel is a success URL; update the counter
        // synchronously for is_full() checks.
        self.counter.fetch_add(1, Ordering::Relaxed);
        self.tx.send(msg).await
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
                },
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
    // Test-only module: every `.unwrap()`/`.expect()` below operates on
    use super::*;
    use url::Url;

    fn make_url(url: &str) -> DiscoveredUrl {
        let u = Url::parse(url).unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        DiscoveredUrl::html(u, 0, parent)
    }

    // =========================================================================
    // Basic functionality
    // =========================================================================

    #[tokio::test]
    async fn test_collector_basic() {
        let collector = ResultsCollector::new(100, Some(200));

        collector
            .send(CrawlMessage::success(make_url("https://a.com")))
            .await
            .unwrap();
        collector
            .send(CrawlMessage::success(make_url("https://b.com")))
            .await
            .unwrap();

        assert_eq!(collector.len(), 2);

        let results = collector.collect().await;
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_collector_is_full() {
        let collector = ResultsCollector::new(100, None);

        for i in 0..5 {
            collector
                .send(CrawlMessage::success(make_url(&format!("https://{i}.com"))))
                .await
                .unwrap();
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
                    let url = make_url(&format!("https://worker{i}-{j}.com"));
                    collector.send(CrawlMessage::success(url)).await.ok();
                }
            });
        }

        while set.join_next().await.is_some() {}

        assert_eq!(collector.len(), 50);

        let results = collector.collect().await;
        assert_eq!(results.len(), 50);
    }

    // =========================================================================
    // Delivery and lifecycle tests
    // =========================================================================

    #[tokio::test]
    async fn test_send_before_collect_delivers_message() {
        // Create collector, send a message, then collect (which drops tx).
        // After collect, the internal worker finishes. We verify the worker
        // received the message by checking the collected results.
        let collector = ResultsCollector::new(100, None);

        collector
            .send(CrawlMessage::success(make_url(
                "https://before-collect.com",
            )))
            .await
            .unwrap();

        let results = collector.collect().await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url.as_str(), "https://before-collect.com/");
    }

    #[tokio::test]
    async fn test_collector_empty_by_default() {
        let collector = ResultsCollector::new(100, None);
        assert!(collector.is_empty());
        assert_eq!(collector.len(), 0);
        assert!(!collector.is_full(1));
    }

    #[tokio::test]
    async fn test_collector_clone_does_not_share_handle() {
        let collector = ResultsCollector::new(100, None);
        let clone = collector.clone();

        // Both can send
        clone
            .send(CrawlMessage::success(make_url("https://clone.com")))
            .await
            .unwrap();
        drop(clone); // Drop clone's tx so channel can close

        // Original can collect
        let results = collector.collect().await;
        assert_eq!(results.len(), 1);
    }
}
