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

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::OnceCell;
use tracing::instrument;

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawl_result_repository::CrawlResultRepositoryImpl;
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::elastic_ingestion::ElasticIngestion;
use crate::application::http_client::{HttpClient, HttpClientConfig};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::clock::SystemClock;
use crate::domain::config::ScraperConfig;
use crate::domain::credentials::CredentialStore;
use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::exporter::{Exporter, ExporterConfig, StateStorePort};
use crate::domain::llm_port::LlmPort;
use crate::domain::note_repository::{NoteRepository, VaultNoteReader};
use crate::domain::ports::HttpClientPort;
use crate::domain::repository::{DynVectorRepository, MultiVectorRepository};
use crate::domain::semantic_cleaner::SemanticCleaner;
use crate::domain::session_port::{SessionPoolConfig, SessionPort};
use crate::domain::text_chunker::TextChunker;
use crate::domain::{repositories::CrawlResultRepository, CrawlerConfig};
use crate::infrastructure::autotuning::ElasticConfig;
use crate::infrastructure::bridge::CpuBridge;
use crate::infrastructure::cpu_pool::RayonCpuPool;
use crate::infrastructure::crawler::resource_downloader::{DownloadConfig, ResourceDownloader};
use crate::infrastructure::export::jsonl_exporter::JsonlExporter;
use crate::infrastructure::export::state_store::StateStore;
use crate::infrastructure::export::vector_exporter::VectorExporter;
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

    /// Vault note reader port (ADR-0012-B sub-slice 3.I, #1071). Unset until
    /// injected — callers fall back to a default-constructed fs adapter.
    /// Always compiled: the trait is a domain port.
    vault_note_reader: OnceCell<Arc<dyn VaultNoteReader>>,

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

/// Composition-root factory for the crawl session pool (ADR-0012-B 3.F, #1075).
///
/// `application::crawler::engine` must not construct the infrastructure
/// concrete `DomainSessionPool` — the layering rule is trait-in-domain,
/// concrete-in-infra, DI-via-Container (ADR-0012-B §2.1), and this file is
/// the permanent allowlist entry that owns the `application → infrastructure`
/// edge. The engine's options flow (`crawl_site_with_options` with
/// `session_pool_enabled`) calls this helper with the domain
/// [`SessionPoolConfig`] DTO and receives the pool erased to the domain
/// [`SessionPort`].
///
/// The domain→infrastructure config mapping lives in
/// [`DomainSessionPool::from_domain_config`], so this function names no new
/// infrastructure path beyond the already-imported concrete.
#[must_use]
pub(crate) fn build_crawl_session_pool(config: SessionPoolConfig) -> Arc<dyn SessionPort> {
    // Log the INPUT DTO: `SessionPort::config()` is not overridden by the
    // concrete (it returns the trait default), so reading it back here would
    // report defaults instead of the actual sizing.
    tracing::debug!(
        pool_size = config.pool_size.get(),
        base_delay_ms = config.base_delay.as_millis(),
        "crawl_session_pool_built"
    );
    let pool = DomainSessionPool::from_domain_config(config, Arc::new(SystemClock));
    Arc::new(pool)
}

/// Composition-root factory for the sitemap parser port (ADR-0012-B sitemap
/// port, follow-up of #1082).
///
/// `application::crawler::sitemap_discovery` must not construct the
/// infrastructure concrete `SitemapParser` — the layering rule is
/// trait-in-domain, concrete-in-infra, DI-via-Container (ADR-0012-B §2.1),
/// and this file is the permanent allowlist entry that owns the
/// `application → infrastructure` edge. Callers pass the domain
/// [`SitemapConfig`](crate::domain::crawler_port::SitemapConfig) DTO and a
/// TLS/H2 profile, and receive the parser erased to the domain
/// [`SitemapParserPort`](crate::domain::crawler_port::sitemap::SitemapParserPort)
/// — mirroring [`build_crawl_session_pool`] (#1077).
///
/// # Errors
///
/// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
/// sitemap fetch client fails to build.
pub(crate) fn build_sitemap_parser(
    config: crate::domain::crawler_port::SitemapConfig,
    profile: wreq_util::Profile,
) -> std::result::Result<
    Arc<dyn crate::domain::crawler_port::sitemap::SitemapParserPort>,
    crate::domain::error::CrawlError,
> {
    Ok(Arc::new(
        crate::infrastructure::crawler::SitemapParser::with_config_and_profile(config, profile)?,
    ))
}

/// Build the crawl robots.txt fetcher as the domain [`RobotsPort`] seam.
///
/// Composition-root helper (ADR-0012-B post-narrow robots slice, mirroring
/// [`build_crawl_session_pool`]): the concrete
/// `infrastructure::crawler::robots_utils::RobotsFetcher` is named only here
/// (and at the `cli/` construction sites, which are outside the layer gate);
/// `application::crawler::engine` consumes the returned `Arc<dyn RobotsPort>`.
pub(crate) fn build_robots_fetcher(
    profile: wreq_util::Profile,
    timeout_secs: u64,
) -> Result<
    Arc<dyn crate::domain::crawler_port::RobotsPort>,
    crate::infrastructure::error::InfraError,
> {
    Ok(Arc::new(
        crate::infrastructure::crawler::robots_utils::RobotsFetcher::new(profile, timeout_secs)?,
    ))
}

/// Composition-root factories for the concrete exporters (ADR-0012-B 3.H).
///
/// `application::export_factory` must not construct the infrastructure
/// concretes `jsonl_exporter::JsonlExporter` / `vector_exporter::VectorExporter`
/// — this file is the permanent allowlist entry that owns the
/// `application → infrastructure` edge (same pattern as
/// [`build_crawl_session_pool`]). Format selection itself stays in
/// `export_factory::create_exporter`.
#[must_use]
pub(crate) fn build_jsonl_exporter(config: ExporterConfig) -> Box<dyn Exporter> {
    Box::new(JsonlExporter::new(config))
}

/// Composition-root factory for the vector exporter concrete
/// (ADR-0012-B 3.H). Mirrors [`build_jsonl_exporter`].
#[must_use]
pub(crate) fn build_vector_exporter(config: ExporterConfig) -> Box<dyn Exporter> {
    Box::new(VectorExporter::new(config))
}

/// Build the HTML link extractor as the domain [`LinkExtractor`] seam.
///
/// Composition-root helper (ADR-0012-B unit 6, mirroring
/// [`build_binary_writer`]): the concrete
/// `infrastructure::crawler::link_extractor::HtmlLinkExtractor` (scraper
/// DOM parsing) is named only here; application code consumes the erased
/// domain trait object.
pub(crate) fn build_link_extractor(
) -> std::sync::Arc<dyn crate::domain::link_extractor::LinkExtractor> {
    std::sync::Arc::new(crate::infrastructure::crawler::link_extractor::HtmlLinkExtractor)
}

/// Build the static (non-JS) HTTP fetcher as the [`StaticFetchPort`] seam.
///
/// Composition-root helper (ADR-0012-B unit 7, mirroring
/// [`build_link_extractor`]): the wreq-backed concrete is named only here.
pub(crate) fn build_static_fetcher(
) -> std::sync::Arc<dyn crate::domain::crawler_port::StaticFetchPort> {
    std::sync::Arc::new(crate::infrastructure::crawler::http_client::StaticHttpFetcher)
}

/// Build the discovery queue as the [`UrlQueuePort`] seam.
///
/// Composition-root helper (ADR-0012-B unit 8, mirroring
/// [`build_static_fetcher`]): the dedup+priority concrete is named only
/// here; scheduler and per-page tasks share the erased port object.
pub(crate) fn build_url_queue() -> std::sync::Arc<dyn crate::domain::crawler_port::UrlQueuePort> {
    std::sync::Arc::new(crate::infrastructure::crawler::url_queue::UrlQueue::new())
}

/// Build the default filesystem binary writer as the [`BinaryWriterPort`]
/// fallback seam.
///
/// Composition-root helper (ADR-0012-B unit 4, mirroring
/// [`build_robots_fetcher`]): the concrete
/// `infrastructure::crawler::binary_writer::FsBinaryWriter` is named only
/// here; `application::crawler::discovery` consumes the returned concrete
/// through the domain `BinaryWriterPort` trait when no writer is injected.
pub(crate) fn build_binary_writer() -> crate::infrastructure::crawler::FsBinaryWriter {
    crate::infrastructure::crawler::FsBinaryWriter::new()
}

/// Build the resume state store as the domain [`StateStorePort`] seam.
///
/// Composition-root factory (ADR-0012-B 3.H, #1097, mirroring
/// [`build_binary_writer`]): the legacy `StateStore` concrete is named only
/// here (and at the `cli/` construction sites, which sit outside the layer
/// gate); `cli` consumes the returned `Arc<dyn StateStorePort>`.
///
/// Creation is lazy and infallible — `StateStore::new` and `set_cache_dir`
/// perform no I/O; the state directory is created later, on `save`.
#[instrument]
pub(crate) fn build_state_store(
    state_dir: PathBuf,
    domain: &str,
) -> Arc<dyn StateStorePort> {
    tracing::info!(state_dir = %state_dir.display(), "creating_state_store");
    let mut store = StateStore::new(domain);
    store.set_cache_dir(state_dir);
    Arc::new(store)
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

        // 6b. SSRF guard — same process-wide registry pattern as the WAF
        //     inspector. Idempotent (#996): keep-first; the fallback is
        //     behaviorally identical, so arming order cannot change behavior.
        crate::domain::ssrf_guard::set_ssrf_guard(Arc::new(
            crate::domain::ssrf_guard::DefaultSsrfGuard,
        ));

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
            vault_note_reader: OnceCell::new(),
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

    /// Get the vault note reader port, if one was injected (#1071).
    ///
    /// Clones the `Arc` (cheap) so callers can hold the port across an
    /// `.await` without borrowing the container (`async-clone-before-await`).
    pub fn vault_note_reader(&self) -> Option<Arc<dyn VaultNoteReader>> {
        self.vault_note_reader.get().cloned()
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

    /// Inject a vault note reader port for filesystem vault reads (#1071).
    ///
    /// Takes `Arc<dyn VaultNoteReader>` — the concrete `VaultFsReader` is
    /// constructed by the binary layer (or defaulted at the call site) and
    /// injected here. Absence stays the default.
    ///
    /// Injection is at-most-once ([`OnceCell`] semantics): a second call keeps
    /// the first reader.
    pub fn with_vault_note_reader(self, reader: Arc<dyn VaultNoteReader>) -> Self {
        let _ = self.vault_note_reader.set(reader);
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

        // 2. CpuBridge wraps the Rayon pool with catch_unwind safety. The DI
        //    root keeps naming the concrete (permanent allowlist entry);
        //    ElasticIngestion holds it behind the domain port (ADR-0012-B 3.E.2).
        let bridge: std::sync::Arc<dyn crate::domain::cpu_executor::CpuExecutorPort> =
            Arc::new(CpuBridge::new(
                cpu_pool,
                Arc::new(crate::infrastructure::content_processing::AggressiveProcessor),
            ));

        // 3. HTTP client for resource downloads (separate from scraping client)
        let client = crate::application::http_client::create_http_client()?;
        let permits = Self::build_ingestion_semaphore_permits(config.ram_budget_bytes);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(permits));

        // 4. Resource downloader with elastic semaphore (byte-weighted backpressure)
        let downloader = Arc::new(ResourceDownloader::with_config(
            semaphore,
            client,
            DownloadConfig {
                max_size_bytes: config.max_resource_bytes,
                ..Default::default()
            },
        ));

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

    // --- SSRF guard wiring (ADR-0012 sub-slice 3.C) ---

    /// Wiring: `Container::new` arms the process-wide SSRF guard registry.
    /// Strict-TDD RED without the step-6b arm (fresh nextest process starts
    /// with the registry unarmed; the fallback does not count as armed).
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_arms_ssrf_guard() {
        let (_tmp, _container) = make_test_container().await;
        assert!(
            crate::domain::ssrf_guard::ssrf_guard_armed(),
            "Container::new debe armar el registry del SsrfGuard"
        );
        // Behavioral proof the armed guard is the full guard: it builds a
        // client end-to-end (redirect policy + validating resolver are
        // applied inside `DefaultSsrfGuard::secure_client`). The concrete
        // type is guaranteed at the arming site by the compiler.
        let builder = wreq::Client::builder();
        let _client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()
            .expect("armed guard builds a client");
    }

    /// Keep-first idempotency (#996): `Container::new` must not replace a
    /// guard the registry already holds. The winner is captured after this
    /// test's own arm rather than assumed, because the registry is
    /// process-global and unresettable: nextest starts each test unarmed (so
    /// the winner is exactly the sentinel and the test also proves it
    /// survives `Container::new`), while plain `cargo test` shares one
    /// process where a sibling may have armed it first.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_ssrf_guard_arming_is_keep_first() {
        #[derive(Debug, Default)]
        struct SentinelGuard;
        impl crate::domain::ssrf_guard::sealed::Sealed for SentinelGuard {}
        impl crate::domain::ssrf_guard::SsrfGuard for SentinelGuard {
            fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
                builder // test sentinel: no guard behavior
            }
        }

        let sentinel: std::sync::Arc<dyn crate::domain::ssrf_guard::SsrfGuard> =
            std::sync::Arc::new(SentinelGuard);
        crate::domain::ssrf_guard::set_ssrf_guard(sentinel);
        // Captured after our own arm, so this is a registry value and not
        // the fallback: the sentinel when this test armed an unarmed
        // registry, or a sibling's guard in a shared `cargo test` process.
        let winner = crate::domain::ssrf_guard::ssrf_guard();

        let (_tmp, _container) = make_test_container().await;

        assert!(
            std::sync::Arc::ptr_eq(&crate::domain::ssrf_guard::ssrf_guard(), &winner),
            "Container::new no debe reemplazar un guard ya armado (keep-first, #996)"
        );
    }

    // --- Session-pool composition-root seam (ADR-0012-B sub-slice 3.F, #1075) ---

    /// `build_crawl_session_pool` is the sanctioned place where the domain
    /// `SessionPoolConfig` DTO becomes an `Arc<dyn SessionPort>`. The returned
    /// port must serve a healthy acquire immediately (pure construction — no
    /// wreq/FFI, so this test runs under Miri too).
    #[test]
    fn build_crawl_session_pool_returns_usable_port() {
        let pool = build_crawl_session_pool(SessionPoolConfig::default());
        assert!(
            pool.acquire("example.com").is_some(),
            "the port built by the composition-root seam must serve a healthy acquire"
        );
    }

    // --- Sitemap-parser composition-root seam (ADR-0012-B, #1082 follow-up) ---

    /// `build_sitemap_parser` is the sanctioned place where the domain
    /// `SitemapConfig` DTO + TLS profile become an `Arc<dyn SitemapParserPort>`.
    /// Unlike the session-pool seam, construction builds the wreq fetch client
    /// (boring-sys2 FFI), so this test follows the established Miri-ignore
    /// pattern of every other client-building container test. The assertion is
    /// that the seam succeeds and the erased port type is constructible; no
    /// network call is made.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn build_sitemap_parser_returns_usable_port() {
        let parser = build_sitemap_parser(
            crate::domain::crawler_port::SitemapConfig::default(),
            wreq_util::Profile::Chrome145,
        );
        assert!(
            parser.is_ok(),
            "the composition-root seam must build an erased sitemap port"
        );
        // The explicit binding proves the erased `Arc<dyn SitemapParserPort>`
        // return is usable (Send + Sync come with the trait's supertraits).
        let _port: Arc<dyn crate::domain::crawler_port::sitemap::SitemapParserPort> =
            parser.unwrap();
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

    // --- Vault note reader port (ADR-0012-B sub-slice 3.I, #1071) ---

    /// In-crate stub reader — proves the seam accepts any
    /// `Arc<dyn VaultNoteReader>` without touching the filesystem.
    #[derive(Debug, Default)]
    struct StubVaultNoteReader;

    impl crate::domain::note_repository::VaultNoteReader for StubVaultNoteReader {
        fn read_vault_notes(
            &self,
            _vault_path: &std::path::Path,
        ) -> Result<Vec<crate::domain::note_repository::VaultNote>, crate::error::ScraperError>
        {
            Ok(Vec::new())
        }
    }

    /// Absence: a freshly built container reports no vault note reader.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn vault_note_reader_absent_by_default() {
        let (_tmp, container) = make_test_container().await;
        assert!(
            container.vault_note_reader().is_none(),
            "vault_note_reader() must be None when no reader was injected"
        );
    }

    /// Injection: `with_vault_note_reader` makes the accessor report present.
    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn with_vault_note_reader_sets_reader() {
        let (_tmp, container) = make_test_container().await;
        let container = container.with_vault_note_reader(Arc::new(StubVaultNoteReader));
        assert!(
            container.vault_note_reader().is_some(),
            "vault_note_reader() must be Some after with_vault_note_reader injection"
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
