//! Elastic ingestion orchestrator — wires the full 7-layer pipeline (Issue #51, PR5).
//!
//! Glues together the PR1–PR4 building blocks into a single fail-fast pipeline
//! per frozen design Decision 2:
//!
//! ```text
//! URL → SHA-256 Check → Semaphore Acquire → HTTP Stream (25MB) →
//!   CpuBridge (Rayon: lol_html + ONNX) → SQLite Persist → Release Permits
//! ```
//!
//! # Architecture (frozen Decision 1: monomorphization)
//!
//! `ElasticIngestion<R: VectorRepository>` is generic over the repository
//! trait and monomorphized at compile time — no `Box<dyn Future>`, no heap
//! allocation for dispatch. This resolves the `async fn`-in-trait non-`Send` /
//! non-`dyn` problem left open by PR4: the repo's native `async fn` methods
//! are awaited on the orchestrator's own task, so no cross-thread `Send` future
//! is ever constructed.
//!
//! # Fail-fast (frozen Decision 3: no internal retries)
//!
//! - CPU panic  → `tracing::error!`, `PermitGuard` drops (RAII), propagate
//!   `ScraperError::Ingestion`. No sleep/backoff.
//! - Network err → `tracing::warn!`, `PermitGuard` drops (RAII), propagate.
//!   Retries are delegated to the top-level URL queue (future work).

use sha2::{Digest, Sha256};
use tracing::{error, info, warn};

use crate::domain::repository::VectorRepository;
use crate::error::ScraperError;
use crate::infrastructure::bridge::CpuBridge;
use crate::infrastructure::config::AutotuningConfig;
use crate::infrastructure::crawler::resource_downloader::{DownloadedResource, ResourceDownloader};

/// Elastic ingestion pipeline orchestrator (frozen Decision 1).
///
/// Generic over the [`VectorRepository`] trait and monomorphized at compile
/// time, so the repo's native `async fn` methods are awaited inline without
/// `Box<dyn Future>` or `dyn` dispatch.
///
/// Construct with [`ElasticIngestion::new`]; optionally inject an ONNX
/// [`SemanticCleaner`] under the `ai` feature via
/// [`ElasticIngestion::with_cleaner`] (frozen Decision 5).
///
/// [`SemanticCleaner`]: crate::domain::semantic_cleaner::SemanticCleaner
pub struct ElasticIngestion<R: VectorRepository + Send + Sync> {
    downloader: ResourceDownloader,
    bridge: CpuBridge,
    repository: R,
    config: AutotuningConfig,
    /// Optional ONNX semantic cleaner (frozen Decision 5). When `Some`, the
    /// orchestrator runs the cleaner's async `clean()` (HTML chunking +
    /// embeddings) instead of the bridge's sync lol_html text extraction.
    /// `None` (and the no-`ai` build) → `embedding = None`.
    ///
    /// Deviation from Decision 1's exact 4-field struct: this 5th field is
    /// feature-gated and unavoidable — `SemanticCleaner::clean()` is async, so
    /// embeddings cannot run in the sync Rayon bridge closure and must live in
    /// the orchestrator's async context. See PR5 apply-progress for the full
    /// rationale.
    #[cfg(feature = "ai")]
    cleaner: Option<std::sync::Arc<dyn crate::domain::semantic_cleaner::SemanticCleaner>>,
}

impl<R: VectorRepository + Send + Sync> ElasticIngestion<R> {
    /// Wire the four pipeline components (frozen Decision 1: monomorphization).
    ///
    /// The repository is generic and monomorphized at compile time — the repo's
    /// native `async fn` methods are awaited inline on the orchestrator's own
    /// task, avoiding the `dyn`/`Send` future problem without `Box<dyn Future>`.
    #[must_use]
    pub fn new(
        downloader: ResourceDownloader,
        bridge: CpuBridge,
        repository: R,
        config: AutotuningConfig,
    ) -> Self {
        Self {
            downloader,
            bridge,
            repository,
            config,
            #[cfg(feature = "ai")]
            cleaner: None,
        }
    }

    /// Inject an ONNX [`SemanticCleaner`] for embedding generation (Decision 5).
    ///
    /// When set, [`run`](Self::run) routes HTML through the cleaner's async
    /// `clean()` (HTML chunking + 384-dim embeddings) instead of the bridge's
    /// sync `lol_html` text extraction. `SemanticCleaner::clean()` is async, so
    /// it cannot run in the sync Rayon bridge closure and must live here.
    ///
    /// [`SemanticCleaner`]: crate::domain::semantic_cleaner::SemanticCleaner
    #[cfg(feature = "ai")]
    #[must_use]
    pub fn with_cleaner(
        mut self,
        cleaner: std::sync::Arc<dyn crate::domain::semantic_cleaner::SemanticCleaner>,
    ) -> Self {
        self.cleaner = Some(cleaner);
        self
    }

    /// Run the full 7-layer pipeline for a single URL (frozen Decision 2).
    ///
    /// Fail-fast (Decision 3): no internal retries/sleep. Network and CPU
    /// errors propagate immediately after logging; `PermitGuard` RAII (inside
    /// the downloader) releases byte-weighted permits on every path.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError`] on network failure, CPU panic, or persistence
    /// failure.
    pub async fn run(&self, url: &str) -> Result<(), ScraperError> {
        info!(
            cpu_cores = self.config.cpu_cores,
            ram_budget_bytes = self.config.ram_budget_bytes,
            %url,
            "iniciando ingestión elástica"
        );

        // Layer 1+3: HTTP download (PR2 byte-weighted semaphore + PermitGuard RAII).
        // Fail-fast: network error → warn + propagate (no retry).
        let bytes = match self.downloader.download(url).await {
            Ok(b) => b,
            Err(e) => {
                warn!(%url, error = %e, "descarga falló: abortando pipeline");
                return Err(e);
            },
        };
        let size = bytes.len() as u64;

        // Layer 2: SHA-256 content hash.
        let hash = sha256_hex(&bytes);

        // Layer: dedup short-circuit (Decision 3) — skip CPU + persist if known.
        if let Some(existing) = self.repository.resource_exists_by_hash(&hash).await? {
            info!(
                %url,
                existing_url = %existing,
                "recurso ya persistido: omitiendo pipeline (dedup)"
            );
            return Ok(());
        }

        // Layer 4: CPU-bound cleaning (Rayon: lol_html via CpuBridge) — OR, under
        // `ai` with a cleaner set, the ONNX semantic pipeline (async embeddings).
        // Fail-fast: CPU panic → error + propagate (no retry).
        let chunks: Vec<crate::infrastructure::bridge::ProcessedChunk> = {
            #[cfg(feature = "ai")]
            {
                if let Some(cleaner) = &self.cleaner {
                    self.cleaner_chunks(cleaner, &bytes).await?
                } else {
                    self.bridge_chunks(url, bytes, size).await?
                }
            }
            #[cfg(not(feature = "ai"))]
            {
                self.bridge_chunks(url, bytes, size).await?
            }
        };

        // Layer 5: SQLite persist (resource + each chunk).
        let title = extract_title(&chunks);
        self.repository
            .save_resource(url, &title, &hash, size)
            .await?;
        for (idx, chunk) in chunks.into_iter().enumerate() {
            let chunk_id = format!("{hash}-{idx}");
            self.repository
                .save_chunk(
                    &chunk_id,
                    url,
                    idx as i64,
                    &chunk.content,
                    chunk.embedding.as_deref(),
                )
                .await?;
        }

        info!(%url, %hash, "ingestión completada");
        Ok(())
    }

    /// Dispatch the downloaded bytes through the CpuBridge (sync lol_html text
    /// extraction, `embedding = None`). Shared by both the `ai`-without-cleaner
    /// path and the no-`ai` build.
    async fn bridge_chunks(
        &self,
        url: &str,
        bytes: Vec<u8>,
        size: u64,
    ) -> Result<Vec<crate::infrastructure::bridge::ProcessedChunk>, ScraperError> {
        let resource = DownloadedResource {
            url: url.to_string(),
            bytes,
            content_type: None,
            size_bytes: size,
        };
        match self.bridge.dispatch_resource(resource).await {
            Ok(Ok(processed)) => Ok(processed.chunks),
            Ok(Err(e)) => {
                error!(%url, error = %e, "procesamiento CPU falló: abortando pipeline");
                Err(e)
            },
            Err(_) => Err(ScraperError::ingestion(
                "canal CPU bridge cerrado prematuramente",
            )),
        }
    }

    /// Route HTML through the ONNX semantic cleaner (async: chunking +
    /// embeddings). Only compiled under `--features ai`; runtime-untested
    /// because `SemanticCleaner` is sealed (no test impl) and
    /// `SemanticCleanerImpl::new` eagerly loads a ~90 MB model.
    #[cfg(feature = "ai")]
    async fn cleaner_chunks(
        &self,
        cleaner: &std::sync::Arc<dyn crate::domain::semantic_cleaner::SemanticCleaner>,
        bytes: &[u8],
    ) -> Result<Vec<crate::infrastructure::bridge::ProcessedChunk>, ScraperError> {
        let html = String::from_utf8_lossy(bytes);
        let doc_chunks = cleaner
            .clean(&html)
            .await
            .map_err(|e| ScraperError::ingestion(format!("limpieza semántica falló: {e}")))?;
        Ok(doc_chunks
            .into_iter()
            .map(|dc| crate::infrastructure::bridge::ProcessedChunk {
                content: dc.content,
                embedding: dc.embeddings,
            })
            .collect())
    }
}

/// SHA-256 hex digest of the bytes (dependency-free hex encoding).
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let hash = hasher.finalize();
    let mut out = String::with_capacity(hash.len() * 2);
    for b in hash {
        use std::fmt::Write;
        // write! into a String is infallible.
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Derive a best-effort title from the first chunk's first line (≤200 chars).
/// Empty when no chunks or an empty first line — the schema allows NULL titles.
fn extract_title(chunks: &[crate::infrastructure::bridge::ProcessedChunk]) -> String {
    chunks
        .first()
        .and_then(|c| c.content.lines().next())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.chars().take(200).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::cpu_pool::RayonCpuPool;
    use crate::infrastructure::crawler::resource_downloader::DownloadConfig;
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Shared in-memory state for the mock repo (Arc so the test keeps a handle
    /// after handing a clone to the orchestrator).
    #[derive(Default)]
    struct RepoState {
        resources: HashMap<String, (String, String, u64)>,
        chunks: Vec<ChunkRecord>,
    }

    /// In-memory chunk record: (id, resource_url, index, content, embedding).
    type ChunkRecord = (String, String, i64, String, Option<Vec<f32>>);

    /// In-memory `VectorRepository` (the trait is NOT sealed, so the crate's
    /// test module can implement it — no SQLite needed for orchestrator unit tests).
    #[derive(Clone, Default)]
    struct InMemoryRepo {
        state: Arc<Mutex<RepoState>>,
    }

    impl VectorRepository for InMemoryRepo {
        fn save_resource<'a>(
            &'a self,
            url: &'a str,
            title: &'a str,
            content_hash: &'a str,
            size_bytes: u64,
        ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
            Box::pin(async move {
                let mut res = self.state.lock().expect("repo mutex poisoned");
                if let Some((existing_url, _, _)) = res.resources.get(content_hash) {
                    return Ok(existing_url.clone());
                }
                res.resources.insert(
                    content_hash.to_string(),
                    (url.to_string(), title.to_string(), size_bytes),
                );
                Ok(url.to_string())
            })
        }

        fn save_chunk<'a>(
            &'a self,
            id: &'a str,
            resource_url: &'a str,
            chunk_index: i64,
            content: &'a str,
            embedding: Option<&'a [f32]>,
        ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
            Box::pin(async move {
                self.state
                    .lock()
                    .expect("repo mutex poisoned")
                    .chunks
                    .push((
                        id.to_string(),
                        resource_url.to_string(),
                        chunk_index,
                        content.to_string(),
                        embedding.map(|e| e.to_vec()),
                    ));
                Ok(())
            })
        }

        fn resource_exists_by_hash<'a>(
            &'a self,
            content_hash: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(self
                    .state
                    .lock()
                    .expect("repo mutex poisoned")
                    .resources
                    .get(content_hash)
                    .map(|(u, _, _)| u.clone()))
            })
        }

        fn get_vector<'a>(
            &'a self,
            _chunk_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>>
        {
            Box::pin(async move { Ok(None) })
        }
    }

    // ====================================================================
    // wreq/HTTP-dependent orchestrator tests
    //
    // Miri interpreta MIR y no puede ejecutar C FFI. make_orchestrator()
    // construye un wreq::Client que depende de boring-sys2 (BoringSSL →
    // TLS_method FFI). Aislar estos tests en un solo bloque #[cfg(not(miri))]
    // evita parchear test por test y mantiene Miri enfocado en detectar UB
    // en la lógica Rust pura.
    // ====================================================================

    #[cfg(not(miri))]
    mod wreq {
        use super::*;

        fn make_orchestrator(repo: InMemoryRepo) -> ElasticIngestion<InMemoryRepo> {
            let client = ::wreq::Client::builder()
                .build()
                .expect("fallo construyendo cliente wreq de prueba");
            let semaphore = Arc::new(tokio::sync::Semaphore::new(1 << 20));
            let downloader = ResourceDownloader::with_config(
                semaphore,
                client,
                DownloadConfig {
                    global_timeout_seconds: 5,
                    chunk_timeout_seconds: 5,
                    max_size_bytes: 1024 * 1024,
                    ..DownloadConfig::default()
                },
            );
            let pool = RayonCpuPool::new(2).expect("pool de 2 hilos");
            let bridge = CpuBridge::new(pool);
            let config = AutotuningConfig {
                cpu_cores: 2,
                ram_budget_bytes: 1 << 20,
            };
            ElasticIngestion::new(downloader, bridge, repo, config)
        }

        async fn serve_html(body: &str) -> (MockServer, String) {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body.as_bytes().to_vec()))
                .mount(&server)
                .await;
            let url = format!("{}/", server.uri());
            (server, url)
        }

        #[tokio::test]
        async fn test_run_persists_resource_and_chunk() {
            let repo = InMemoryRepo::default();
            let orc = make_orchestrator(repo.clone());
            let (_server, url) =
                serve_html("<nav>menu</nav><main><p>hello elastic world</p></main>").await;

            orc.run(&url).await.expect("pipeline debe completarse");

            let state = repo.state.lock().expect("repo mutex poisoned");
            assert_eq!(
                state.resources.len(),
                1,
                "exactamente un recurso persistido"
            );
            let (_hash, (saved_url, _title, size)) =
                state.resources.iter().next().expect("un recurso");
            assert_eq!(saved_url, &url);
            assert!(*size > 0, "size_bytes debe ser positivo");
            assert_eq!(state.chunks.len(), 1, "exactamente un chunk persistido");
            let chunk = &state.chunks[0];
            assert_eq!(chunk.1, url, "chunk enlaza al recurso correcto");
            assert_eq!(chunk.2, 0, "primer chunk_index == 0");
            assert!(
                chunk.3.contains("hello elastic world"),
                "contenido limpio persistido: {}",
                chunk.3
            );
            assert!(
                !chunk.3.contains("menu"),
                "el boilerplate (nav) no debe persistir: {}",
                chunk.3
            );
            assert!(
                chunk.4.is_none(),
                "sin limpiador ONNX, embedding debe ser None"
            );
        }

        #[tokio::test]
        async fn test_run_dedup_short_circuits_when_hash_exists() {
            let repo = InMemoryRepo::default();
            let orc = make_orchestrator(repo.clone());
            let (_server, url) = serve_html("<main><p>duplicate content</p></main>").await;

            orc.run(&url).await.expect("primera ingestión ok");
            let after_first = {
                let s = repo.state.lock().expect("poisoned");
                (s.resources.len(), s.chunks.len())
            };
            assert_eq!(after_first, (1, 1));

            orc.run(&url).await.expect("segunda ingestión (dedup) ok");
            let after_second = {
                let s = repo.state.lock().expect("poisoned");
                (s.resources.len(), s.chunks.len())
            };
            assert_eq!(
                after_second,
                (1, 1),
                "dedup debe impedir filas duplicadas para el mismo content_hash"
            );
        }

        #[tokio::test]
        async fn test_run_network_error_propagates_without_retry() {
            let repo = InMemoryRepo::default();
            let orc = make_orchestrator(repo.clone());
            let port = {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
                listener.local_addr().expect("addr").port()
            };
            let url = format!("http://127.0.0.1:{port}/");

            let result = orc.run(&url).await;
            assert!(result.is_err(), "error de red debe propagarse (fail-fast)");
            let state = repo.state.lock().expect("repo mutex poisoned");
            assert!(
                state.resources.is_empty() && state.chunks.is_empty(),
                "ningún recurso/chunk debe persistirse tras un fallo de red"
            );
        }
    }

    // ---- Task 5.1: static Send + Sync (orchestrator is shareable) ----

    #[test]
    fn test_elastic_ingestion_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ElasticIngestion<InMemoryRepo>>();
    }

    /// Integrity: the content-hash producer emits lowercase hex (no uppercase),
    /// and matches a known SHA-256 vector. This is the value upstream feeds into
    /// `StreamRepository`'s `{hash}-{index}` chunk id, so it must stay lowercase.
    #[test]
    fn contract_sha256_hex_producer_is_lowercase() {
        // SHA-256 of the empty input (well-known vector), all lowercase hex.
        let empty = sha256_hex(&[]);
        assert_eq!(
            empty, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "must match the known empty-input SHA-256 digest"
        );

        let is_lowercase_hex = empty
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            && empty.len() == 64;
        assert!(
            is_lowercase_hex,
            "producer output must be 64 lowercase-hex chars, got: {empty}"
        );

        // Non-empty input also stays lowercase hex and is 64 chars.
        let sample = sha256_hex(b"webfang");
        assert_eq!(sample.len(), 64, "SHA-256 digest is always 64 hex chars");
        assert!(
            sample
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "non-empty digest must also be lowercase hex, got: {sample}"
        );
    }
}
