//! Dependency Injection Container
//!
//! Provides a centralized way to wire up all services and their dependencies.
//! Following Clean Architecture, the container lives in the application layer
//! and creates instances of infrastructure implementations.
//!
//! # Design (Phase 3)
//!
//! The Container is the **single resolution point** for all services. It holds:
//! - Configuration (crawler + scraper)
//! - Port-trait objects for infrastructure (HTTP, export, persistence)
//! - Application services (rate limiter, deduplicator, credentials)
//!
//! Port traits are defined in `domain::ports` — the domain layer owns the
//! abstractions. The Container creates real infrastructure implementations
//! and stores them as `Arc<dyn Port>`.

use std::sync::Arc;
use tokio::sync::OnceCell;

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawl_result_repository::CrawlResultRepositoryImpl;
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::elastic_ingestion::ElasticIngestion;
use crate::application::http_client::{HttpClient, HttpClientConfig};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::config::ScraperConfig;
use crate::domain::credentials::CredentialStore;
use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::llm_port::LlmPort;
use crate::domain::note_repository::NoteRepository;
use crate::domain::ports::HttpClientPort;
use crate::domain::repository::{DynVectorRepository, MultiVectorRepository};
use crate::domain::semantic_cleaner::SemanticCleaner;
use crate::domain::text_chunker::TextChunker;
use crate::domain::{repositories::CrawlResultRepository, CrawlerConfig};
use crate::infrastructure::autotuning::ElasticConfig;
use crate::infrastructure::bridge::CpuBridge;
use crate::infrastructure::cpu_pool::RayonCpuPool;
use crate::infrastructure::crawler::resource_downloader::{DownloadConfig, ResourceDownloader};
use crate::infrastructure::export::state_store::StateStore;
use crate::infrastructure::http::waf_engine::WafInspector;
use crate::infrastructure::network::session_pool::DomainSessionPool;
// SQLite persistence layer — only compiled under the `persistence` feature.
#[cfg(feature = "persistence")]
use crate::infrastructure::persistence::sqlite::{
    self as sqlite_persistence, SqliteVectorRepository,
};

/// Dependency Injection Container
///
/// Holds all service instances and their configurations.
/// Services are created once and reused throughout the application.
///
/// # Architecture
///
/// - Port-trait objects (`Arc<dyn Trait>`) for infrastructure abstractions
/// - Concrete types for application services that don't need swapping
/// - Builder methods (`with_*`) for optional services
#[derive(Clone)]
pub struct Container {
    /// Configuration for scraping behavior
    pub scraper_config: ScraperConfig,

    // --- Port-trait objects (domain abstractions) ---
    /// HTTP client behind port trait — application code depends on the trait, not wreq
    http_client: Arc<dyn HttpClientPort>,

    // --- Application services (concrete, Arc-shared) ---
    /// Rate limiter for crawl operations
    rate_limiter: Option<Arc<SharedRateLimiter>>,
    /// URL deduplication (lock-free, DashSet-backed)
    deduplicator: Arc<UrlDeduplicator>,
    /// Credential store for API keys and tokens
    credential_store: Arc<CredentialStore>,

    // --- Infrastructure services (optional, feature-gated) ---
    /// State store for resume functionality
    pub state_store: Option<Arc<StateStore>>,
    /// Crawl result repository (append-only log)
    pub crawl_result_repo: Option<Arc<dyn CrawlResultRepository>>,
    /// Elastic ingestion pipeline (optional, activated via `--elastic` or
    /// `--output-vectors`). Erased to `DynVectorRepository` so it can hold either
    /// the SQLite repo (`persistence` feature) or the headless `StreamRepository`
    /// JSONL sink.
    pub elastic_ingestion: Option<Arc<ElasticIngestion<DynVectorRepository>>>,

    // --- Optional domain ports (injected post-construction) ---
    /// AI semantic cleaner (optional). Unset when the `ai` feature is off or
    /// no cleaner has been injected yet; MCP tools map this absence to an
    /// honest error. Stored as `Arc<dyn SemanticCleaner>` — the trait is a
    /// domain port like `HttpClientPort`, always compiled
    /// (no `#[cfg(feature = "ai")]`).
    ///
    /// Held in a [`OnceCell`] so the injection is OBSERVABLE through a shared
    /// `&Container` (lazy post-construction wiring, #759): the MCP binaries
    /// share one `Arc<Container>` with a background warmup task that injects
    /// the ports after the server handshake has started. `OnceCell` guarantees
    /// at-most-once injection; a second `set` is a no-op.
    cleaner: OnceCell<Arc<dyn SemanticCleaner>>,

    /// Embedding port for text vectorization (#386). Unset when the `ai`
    /// feature is off. Always compiled — the trait is a domain port.
    embedding_port: OnceCell<Arc<dyn EmbeddingPort>>,

    /// Note repository for vault search persistence (#386). Unset when vault
    /// search is not configured.
    note_repository: OnceCell<Arc<dyn NoteRepository>>,

    /// Text chunker for Markdown segmentation (#386). Unset when the `ai`
    /// feature is off. Always compiled — the trait is a domain port.
    text_chunker: OnceCell<Arc<dyn TextChunker>>,

    /// LLM completion port for structured extraction (#789). Unset when the
    /// `ai` feature is off or no provider is configured; the service maps
    /// this absence to an honest Config error. Always compiled — the trait
    /// is a domain port.
    llm_port: OnceCell<Arc<dyn LlmPort>>,
}

/// Vault-search AI ports (#433), constructed in the binary layer (CLI/MCP) and
/// injected into the [`Container`] in one shot.
///
/// Fields are domain trait objects so the dependency direction (`ai → core`) is
/// respected: `webfang_core` never names the concrete adapters, which live in
/// `webfang_ai` / the persistence layer and are assembled by the binary crate.
/// An empty bundle (the [`Default`]) wires nothing — used by builds without the
/// `ai` feature.
#[derive(Clone, Default)]
pub struct VaultAiPorts {
    /// Embedding port for query/chunk vectorization.
    pub embedding_port: Option<Arc<dyn EmbeddingPort>>,
    /// Note repository for vault search persistence.
    pub note_repository: Option<Arc<dyn NoteRepository>>,
    /// Text chunker for Markdown segmentation.
    pub text_chunker: Option<Arc<dyn TextChunker>>,

    /// Semantic cleaner for AI-driven content extraction. `None` when the `ai`
    /// feature is off or no cleaner was injected.
    pub cleaner: Option<Arc<dyn SemanticCleaner>>,
}

impl Container {
    /// Create a new container with the given configurations.
    ///
    /// Initializes all core services. Optional services (state_store,
    /// elastic_ingestion) are set to `None` and can be activated via builder
    /// methods; the AI vault ports start as unset `OnceCell`s and can be
    /// injected through the `with_*`/`inject_vault_ports` methods.
    ///
    /// # Arguments
    ///
    /// * `crawler_config` - Configuration for crawling behavior
    /// * `scraper_config` - Configuration for scraping behavior
    ///
    /// # Returns
    ///
    /// A new container instance with all core services initialized.
    pub async fn new(
        _crawler_config: CrawlerConfig,
        scraper_config: ScraperConfig,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // 1. HTTP client — concrete HttpClient behind port trait
        // The application layer depends on `HttpClientPort`; the production
        // `HttpClient` impl is stored as the trait object. No concrete
        // `wreq::Client` is exposed — raw HTTP stays behind the port.
        let session_pool = Arc::new(DomainSessionPool::default_pool());
        let http_client_inner = HttpClient::new(HttpClientConfig::default())?
            .with_session_pool(session_pool as Arc<dyn crate::domain::session_port::SessionPort>);
        let http_client: Arc<dyn HttpClientPort> = Arc::new(http_client_inner);

        // 2. Rate limiter (optional — failure is non-fatal)
        let rate_limiter = match SharedRateLimiter::new(&RateLimiterConfig::default()) {
            Ok(rl) => Some(Arc::new(rl)),
            Err(e) => {
                tracing::warn!("rate limiter init failed: {e}");
                None
            },
        };

        // 3. URL deduplicator
        let deduplicator = Arc::new(UrlDeduplicator::new());

        // 4. Credential store (empty by default)
        let credential_store = Arc::new(CredentialStore::new());

        // 5. Crawl result repository (append-only log)
        let log_path = scraper_config.output_dir.join("crawl_results.bin");
        let crawl_result_repo = match CrawlResultRepositoryImpl::new(log_path, 1024) {
            Ok(repo) => Some(Arc::new(repo) as Arc<dyn CrawlResultRepository>),
            Err(e) => {
                tracing::warn!("failed to initialize repository: {e}");
                None
            },
        };

        // 6. WAF inspector — install the process-wide static the application
        //    call sites consume via `waf_inspector()`. Idempotent (#996): a
        //    second container build keeps the first value.
        crate::domain::waf::set_waf_inspector(
            Arc::new(WafInspector) as Arc<dyn crate::domain::waf::WafInspectorPort>
        );

        Ok(Self {
            scraper_config,
            http_client,
            rate_limiter,
            deduplicator,
            credential_store,
            state_store: None,
            crawl_result_repo,
            elastic_ingestion: None,
            cleaner: OnceCell::new(),
            embedding_port: OnceCell::new(),
            note_repository: OnceCell::new(),
            text_chunker: OnceCell::new(),
            llm_port: OnceCell::new(),
        })
    }

    // ========================================================================
    // Accessor methods
    // ========================================================================

    /// Get the scraper configuration.
    #[must_use]
    pub fn config(&self) -> &ScraperConfig {
        &self.scraper_config
    }

    /// Get the HTTP client port — application code uses the trait, not wreq.
    #[must_use]
    pub fn http_client(&self) -> &Arc<dyn HttpClientPort> {
        &self.http_client
    }

    /// Get the rate limiter, if successfully initialized.
    #[must_use]
    pub fn rate_limiter(&self) -> Option<&Arc<SharedRateLimiter>> {
        self.rate_limiter.as_ref()
    }

    /// Get the URL deduplicator.
    #[must_use]
    pub fn deduplicator(&self) -> &Arc<UrlDeduplicator> {
        &self.deduplicator
    }

    /// Get the credential store.
    #[must_use]
    pub fn credential_store(&self) -> &Arc<CredentialStore> {
        &self.credential_store
    }

    /// Get a repository for crawl results (backed by append-only log).
    pub fn crawl_result_repository(&self) -> Option<Arc<dyn CrawlResultRepository>> {
        self.crawl_result_repo.clone()
    }

    /// Get the semantic cleaner port, if one was injected.
    ///
    /// Clones the `Arc` (cheap) so callers can hold the cleaner across an
    /// `.await` without borrowing the container (`async-clone-before-await`).
    pub fn cleaner(&self) -> Option<Arc<dyn SemanticCleaner>> {
        self.cleaner.get().cloned()
    }

    /// Get the embedding port, if one was injected (#386).
    ///
    /// Clones the `Arc` (cheap) so callers can hold the port across an
    /// `.await` without borrowing the container.
    pub fn embedding_port(&self) -> Option<Arc<dyn EmbeddingPort>> {
        self.embedding_port.get().cloned()
    }

    /// Get the note repository, if one was injected (#386).
    pub fn note_repository(&self) -> Option<Arc<dyn NoteRepository>> {
        self.note_repository.get().cloned()
    }

    /// Get the text chunker, if one was injected (#386).
    pub fn text_chunker(&self) -> Option<Arc<dyn TextChunker>> {
        self.text_chunker.get().cloned()
    }

    /// Get the LLM completion port, if one was injected (#789).
    ///
    /// Clones the `Arc` (cheap) so callers can hold the port across an
    /// `.await` without borrowing the container (`async-clone-before-await`).
    pub fn llm_port(&self) -> Option<Arc<dyn LlmPort>> {
        self.llm_port.get().cloned()
    }

    /// Access the elastic ingestion pipeline, if activated.
    #[must_use]
    pub fn elastic_ingestion(&self) -> Option<&ElasticIngestion<DynVectorRepository>> {
        self.elastic_ingestion.as_deref()
    }

    // ========================================================================
    // Builder methods for optional services
    // ========================================================================

    /// Set the state store for resume functionality.
    pub fn with_state_store(mut self, state_store: StateStore) -> Self {
        self.state_store = Some(Arc::new(state_store));
        self
    }

    /// Set a pre-configured rate limiter (overrides the default).
    pub fn with_rate_limiter(mut self, limiter: SharedRateLimiter) -> Self {
        self.rate_limiter = Some(Arc::new(limiter));
        self
    }

    /// Set a pre-configured credential store.
    pub fn with_credential_store(mut self, store: CredentialStore) -> Self {
        self.credential_store = Arc::new(store);
        self
    }

    /// Inject an AI semantic cleaner (domain port).
    ///
    /// Takes `Arc<dyn SemanticCleaner>` directly — mirrors the CLI construction
    /// `Arc::new(cleaner) as Arc<dyn SemanticCleaner>` and the
    /// `McpState::with_inspector` precedent. Absence stays the default.
    ///
    /// Injection is at-most-once ([`OnceCell`] semantics): a second call keeps
    /// the first cleaner.
    pub fn with_cleaner(self, cleaner: Arc<dyn SemanticCleaner>) -> Self {
        let _ = self.cleaner.set(cleaner);
        self
    }

    /// Inject an embedding port for text vectorization (#386).
    ///
    /// Takes `Arc<dyn EmbeddingPort>` — the concrete `InferencePool` wrapper
    /// is constructed in the CLI/MCP layer and injected here.
    ///
    /// Injection is at-most-once ([`OnceCell`] semantics): a second call keeps
    /// the first port.
    pub fn with_embedding_port(self, port: Arc<dyn EmbeddingPort>) -> Self {
        let _ = self.embedding_port.set(port);
        self
    }

    /// Inject a note repository for vault search persistence (#386).
    ///
    /// Injection is at-most-once ([`OnceCell`] semantics): a second call keeps
    /// the first repository.
    pub fn with_note_repository(self, repo: Arc<dyn NoteRepository>) -> Self {
        let _ = self.note_repository.set(repo);
        self
    }

    /// Inject a text chunker for Markdown segmentation (#386).
    ///
    /// Injection is at-most-once ([`OnceCell`] semantics): a second call keeps
    /// the first chunker.
    pub fn with_text_chunker(self, chunker: Arc<dyn TextChunker>) -> Self {
        let _ = self.text_chunker.set(chunker);
        self
    }

    /// Inject an LLM completion port for structured extraction (#789).
    ///
    /// Takes `Arc<dyn LlmPort>` — the concrete `OpenAiLlmClient` is built in
    /// the CLI/MCP layer and injected here. Absence stays the default no-op.
    ///
    /// Injection is at-most-once ([`OnceCell`] semantics): a second call keeps
    /// the first port.
    pub fn with_llm_port(self, port: Arc<dyn LlmPort>) -> Self {
        let _ = self.llm_port.set(port);
        self
    }

    /// Inject the vault-search AI ports that are present in `ports` (#433).
    ///
    /// Each field is wired independently; `None` fields leave the corresponding
    /// port untouched. Convenience wrapper over [`inject_vault_ports`](Self::inject_vault_ports)
    /// for the binary composition roots (CLI/MCP) that assemble the whole
    /// bundle at once.
    #[must_use]
    pub fn with_vault_ports(self, ports: VaultAiPorts) -> Self {
        self.inject_vault_ports(ports);
        self
    }

    /// Post-construction injector for the vault-search AI ports (lazy MCP AI
    /// wiring, #759).
    ///
    /// Wires each `Some` field through interior mutability (`&self`): the MCP
    /// binaries share one `Arc<Container>` with a background warmup task that
    /// injects the ports AFTER the server handshake has started, and the
    /// injection must be OBSERVABLE through that shared container. `None`
    /// fields leave the corresponding port untouched. Injection is
    /// at-most-once ([`OnceCell`] semantics): for a port that is already set,
    /// the existing value wins.
    pub fn inject_vault_ports(&self, ports: VaultAiPorts) {
        if let Some(embedding_port) = ports.embedding_port {
            let _ = self.embedding_port.set(embedding_port);
        }
        if let Some(note_repository) = ports.note_repository {
            let _ = self.note_repository.set(note_repository);
        }
        if let Some(text_chunker) = ports.text_chunker {
            let _ = self.text_chunker.set(text_chunker);
        }
        if let Some(cleaner) = ports.cleaner {
            let _ = self.cleaner.set(cleaner);
        }
    }

    /// Build the elastic ingestion pipeline around an arbitrary repository.
    ///
    /// Shared by the SQLite path (`persistence` feature) and the headless
    /// `StreamRepository` JSONL sink. Wires `RayonCpuPool` → `CpuBridge` →
    /// `ResourceDownloader` (byte-weighted semaphore) → `ElasticIngestion`.
    ///
    /// Size the elastic-ingestion `Semaphore` in **BYTES** — the unit that
    /// `ResourceDownloader` consumes via `acquire_many(chunk_len)` (one permit
    /// == one byte). The RAM budget is the permit budget expressed directly in
    /// bytes, so the semaphore total MUST equal `ram_budget_bytes` (capped at
    /// `Semaphore::MAX_PERMITS`). This is the contract that prevents the #544
    /// deadlock: had the semaphore been sized in *count* of resources
    /// (`ram_budget / max_resource`), a single multi-byte chunk could request
    /// more permits than exist and `acquire_many` would enqueue a partial grant
    /// and wait forever (unbounded), hanging the pipeline.
    #[must_use]
    pub(crate) fn build_ingestion_semaphore_permits(ram_budget_bytes: u64) -> usize {
        usize::try_from(ram_budget_bytes)
            .unwrap_or(usize::MAX)
            .min(tokio::sync::Semaphore::MAX_PERMITS)
    }

    /// # Errors
    ///
    /// Returns an error if the Rayon pool or HTTP client fails to initialize.
    fn build_elastic(
        repository: DynVectorRepository,
        config: &ElasticConfig,
    ) -> Result<ElasticIngestion<DynVectorRepository>, Box<dyn std::error::Error + Send + Sync>>
    {
        // 1. Rayon CPU pool for lol_html processing
        let cpu_pool = RayonCpuPool::new(config.cpu_cores)?;

        // 2. CpuBridge wraps the Rayon pool with catch_unwind safety
        let bridge = CpuBridge::new(
            cpu_pool,
            Arc::new(crate::infrastructure::content_processing::AggressiveProcessor),
        );

        // 3. HTTP client for resource downloads (separate from scraping client)
        let client = crate::application::http_client::create_http_client()?;
        let permits = Self::build_ingestion_semaphore_permits(config.ram_budget_bytes);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(permits));

        // 4. Resource downloader with elastic semaphore (byte-weighted backpressure)
        let downloader = ResourceDownloader::with_config(
            semaphore,
            client,
            DownloadConfig {
                max_size_bytes: config.max_resource_bytes,
                ..Default::default()
            },
        );

        // 5. Assemble pipeline — ElasticIngestion erased to DynVectorRepository
        let autotune = crate::infrastructure::config::AutotuningConfig::from_elastic(config);
        Ok(ElasticIngestion::new(
            downloader, bridge, repository, autotune,
        ))
    }

    /// Build an [`ElasticIngestion`] and attach the AI semantic cleaner when
    /// present under the `ai` feature. Shared by [`with_elastic_ingestion`]
    /// so the injection logic lives in exactly one place.
    fn build_ingestion(
        repository: DynVectorRepository,
        config: &ElasticConfig,
        _cleaner: Option<Arc<dyn SemanticCleaner>>,
    ) -> Result<ElasticIngestion<DynVectorRepository>, Box<dyn std::error::Error + Send + Sync>>
    {
        let ingestion = Self::build_elastic(repository, config)?;
        #[cfg(feature = "ai")]
        let ingestion = if let Some(cleaner) = _cleaner {
            ingestion.with_cleaner(cleaner)
        } else {
            ingestion
        };
        Ok(ingestion)
    }

    /// Activate elastic ingestion across every requested vector sink at once.
    ///
    /// `--elastic` (SQLite persistence) and `--output-vectors` (dependency-free
    /// JSONL stream) are orthogonal **data destinations**, so this wires them
    /// into a single `ElasticIngestion` over a [`MultiVectorRepository`]
    /// fan-out — replacing the former mutually-exclusive `with_elastic` /
    /// `with_stream` branch that silently dropped the JSONL sink (issue #636).
    ///
    /// Returns `self` unchanged (ingestion stays `None`) when no sink is active.
    ///
    /// # Errors
    ///
    /// Returns an error if a SQLite pool cannot be created (under `persistence`
    /// with `--elastic`), the JSONL output cannot be opened, or the Rayon pool /
    /// HTTP client fails to initialize.
    pub(crate) async fn with_elastic_ingestion(
        mut self,
        opts: &CrawlOptions,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let overrides = crate::infrastructure::autotuning::ElasticOverrides {
            cpu_cores: opts.elastic.cpu_cores,
            ram_budget_bytes: opts.elastic.ram_budget_bytes,
            max_resource_bytes: opts.elastic.max_resource_bytes,
            db_path: opts.elastic.db_path.clone(),
        };
        let config = ElasticConfig::resolve(&overrides);

        // Collect every active sink — both can coexist (orthogonal destinations).
        let mut repos: Vec<DynVectorRepository> = Vec::new();

        #[cfg(feature = "persistence")]
        if opts.elastic.enabled {
            let pool = sqlite_persistence::create_pool(&config.db_path, config.db_pool_size)?;
            sqlite_persistence::setup_schema(&pool).await?;
            tracing::info!(db_path = %config.db_path.display(), "elastic_sqlite_sink_wired");
            repos.push(Arc::new(SqliteVectorRepository::new(pool)));
        }

        if let Some(ref path) = opts.elastic.output_vectors {
            repos.push(Arc::new(
                crate::infrastructure::stream::StreamRepository::new(path)?,
            ));
        }

        // No sink requested → leave the ingestion untouched (`None`).
        if repos.is_empty() {
            return Ok(self);
        }

        // One sink → use it directly; multiple → fan out via MultiVectorRepository.
        let repository: DynVectorRepository = match repos.len() {
            1 => repos.pop().ok_or("no repository to activate")?,
            _ => Arc::new(MultiVectorRepository::new(repos)),
        };

        let ingestion = Self::build_ingestion(repository, &config, self.cleaner.get().cloned())?;
        self.elastic_ingestion = Some(Arc::new(ingestion));
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::config::ScraperConfig;
    use crate::domain::CrawlerConfig;
    use crate::infrastructure::autotuning::{ElasticConfig, ElasticOverrides};
    use tempfile::TempDir;

    /// Create a Container with default configs backed by a TempDir.
    /// Returns `(TempDir, Container)` — caller keeps `tmp` alive for the test scope.
    async fn make_test_container() -> (TempDir, Container) {
        let tmp = TempDir::new().unwrap();
        let crawler_config = CrawlerConfig::new(url::Url::parse("https://example.com").unwrap());
        let scraper_config = ScraperConfig {
            output_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let container = Container::new(crawler_config, scraper_config)
            .await
            .unwrap();
        (tmp, container)
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_wires_crawl_result_repository() {
        let (_tmp, container) = make_test_container().await;
        let repo = container.crawl_result_repository();
        assert!(
            repo.is_some(),
            "crawl_result_repository() debe retornar Some"
        );
    }

    // --- Tests for expanded Container (Phase 3: DI with port/adapter) ---

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_provides_all_required_services() {
        let (_tmp, container) = make_test_container().await;

        // Verify all core services are available (non-optional accessors)
        let _ = container.http_client();
        let _ = container.deduplicator();
        let _ = container.credential_store();
        // Optional services
        assert!(
            container.rate_limiter().is_some(),
            "rate_limiter must be available"
        );
        assert!(
            container.crawl_result_repository().is_some(),
            "crawl_result_repository must be available"
        );
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_http_client_implements_port() {
        let (_tmp, container) = make_test_container().await;

        // Verify http_client is usable as a port trait object
        let client = container.http_client();
        let _port: &dyn crate::domain::ports::HttpClientPort = client.as_ref();
        // If this compiles, the port trait is properly implemented
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_config_accessors() {
        let (tmp, container) = make_test_container().await;

        // Verify config accessors work
        assert_eq!(
            container.config().output_dir,
            tmp.path(),
            "config should expose output_dir"
        );
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_clone_shares_services() {
        let (_tmp, container) = make_test_container().await;
        let container2 = container.clone();

        // Both clones share the same Arc'd services
        assert!(Arc::ptr_eq(
            container.http_client(),
            container2.http_client()
        ));
    }

    // --- Tests for the optional semantic-cleaner port (REQ-05) ---

    /// In-crate fake cleaner — the defining crate may implement its own sealed
    /// trait (`private::Sealed`), which external crates cannot. Enables a CI
    /// test of the Container cleaner port without loading an ONNX model.
    struct FakeCleaner;

    impl crate::domain::semantic_cleaner::private::Sealed for FakeCleaner {}

    impl crate::domain::semantic_cleaner::SemanticCleaner for FakeCleaner {
        fn clean<'a>(
            &'a self,
            _url: &'a str,
            _html: &'a str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<
                            Vec<crate::domain::DocumentChunk>,
                            crate::error::SemanticError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn max_tokens(&self) -> usize {
            512
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    /// REQ-05 (absence): a freshly built container reports no cleaner.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn cleaner_absent_by_default() {
        let (_tmp, container) = make_test_container().await;
        assert!(
            container.cleaner().is_none(),
            "cleaner() must be None when no cleaner was injected"
        );
    }

    /// REQ-05 (injection): `with_cleaner` makes the accessor report present.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn with_cleaner_sets_cleaner() {
        let (_tmp, container) = make_test_container().await;
        let container = container.with_cleaner(Arc::new(FakeCleaner));
        assert!(
            container.cleaner().is_some(),
            "cleaner() must be Some after with_cleaner injection"
        );
    }

    /// Stub LLM port — answers any request with an empty records object.
    struct StubLlm;

    impl crate::domain::llm_port::LlmPort for StubLlm {
        fn send_completion<'a>(
            &'a self,
            _request: crate::domain::llm_port::LlmRequest,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = std::result::Result<
                            crate::domain::llm_port::LlmResponse,
                            crate::error::ScraperError,
                        >,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async {
                Ok(crate::domain::llm_port::LlmResponse {
                    content: "{}".into(),
                    input_tokens: 1,
                    output_tokens: 1,
                })
            })
        }
    }

    /// #789 (absence): a freshly built container reports no LLM port.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn llm_port_absent_by_default() {
        let (_tmp, container) = make_test_container().await;
        assert!(
            container.llm_port().is_none(),
            "llm_port() must be None when no port was injected"
        );
    }

    /// #789 (injection): `with_llm_port` makes the accessor report present.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn with_llm_port_sets_llm_port() {
        let (_tmp, container) = make_test_container().await;
        let container = container.with_llm_port(Arc::new(StubLlm));
        assert!(
            container.llm_port().is_some(),
            "llm_port() must be Some after with_llm_port injection"
        );
    }

    /// Regression for #544: the elastic-ingestion semaphore MUST be sized in
    /// BYTES (the unit `ResourceDownloader` consumes via `acquire_many`), not in
    /// a count of resources. With explicit overrides we fix the RAM budget and
    /// assert the produced permit count covers `max_resource_bytes`.
    ///
    /// On the buggy wiring `permits = ram_budget/max_resource` (= 81 for these
    /// values) which is smaller than `max_resource_bytes` (26_214_400) — the
    /// assertion fails. After the fix `permits == ram_budget_bytes` (in bytes),
    /// which is larger, so it passes.
    #[test]
    fn test_elastic_semaphore_sized_in_bytes() {
        let overrides = ElasticOverrides {
            cpu_cores: None,
            ram_budget_bytes: Some(2 * 1024 * 1024 * 1024), // 2 GiB
            max_resource_bytes: Some(25 * 1024 * 1024),     // 25 MiB
            db_path: None,
        };
        let config = ElasticConfig::resolve(&overrides);
        let permits = Container::build_ingestion_semaphore_permits(config.ram_budget_bytes);

        assert!(
            permits >= config.max_resource_bytes as usize,
            "el semáforo debe inicializarse en BYTES (permits={permits}) >= \
             max_resource_bytes={}",
            config.max_resource_bytes
        );
        // Sanity: with 2 GiB budget the permit count equals the byte budget.
        assert_eq!(permits, 2 * 1024 * 1024 * 1024);
    }
}
