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

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawl_result_repository::CrawlResultRepositoryImpl;
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::elastic_ingestion::ElasticIngestion;
use crate::application::http_client::{HttpClient, HttpClientConfig};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::credentials::CredentialStore;
use crate::domain::ports::HttpClientPort;
use crate::domain::repository::DynVectorRepository;
use crate::domain::{repositories::CrawlResultRepository, CrawlerConfig};
use crate::infrastructure::autotuning::ElasticConfig;
use crate::infrastructure::bridge::CpuBridge;
use crate::infrastructure::config::ScraperConfig;
use crate::infrastructure::cpu_pool::RayonCpuPool;
use crate::infrastructure::crawler::resource_downloader::{DownloadConfig, ResourceDownloader};
use crate::infrastructure::export::state_store::StateStore;
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
}

impl Container {
    /// Create a new container with the given configurations.
    ///
    /// Initializes all core services. Optional services (state_store,
    /// elastic_ingestion) are set to `None` and can be activated via builder
    /// methods.
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
        let http_client_inner = HttpClient::new(HttpClientConfig::default())?;
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
                tracing::warn!("no se pudo inicializar el repositorio: {e}");
                None
            },
        };

        Ok(Self {
            scraper_config,
            http_client,
            rate_limiter,
            deduplicator,
            credential_store,
            state_store: None,
            crawl_result_repo,
            elastic_ingestion: None,
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

    /// Build the elastic ingestion pipeline around an arbitrary repository.
    ///
    /// Shared by the SQLite path (`persistence` feature) and the headless
    /// `StreamRepository` JSONL sink. Wires `RayonCpuPool` → `CpuBridge` →
    /// `ResourceDownloader` (byte-weighted semaphore) → `ElasticIngestion`.
    ///
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
        let bridge = CpuBridge::new(cpu_pool);

        // 3. HTTP client for resource downloads (separate from scraping client)
        let client = crate::application::http_client::create_http_client()?;
        let max_concurrent = (config.ram_budget_bytes / config.max_resource_bytes).max(1) as usize;
        let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));

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
            downloader,
            bridge,
            Arc::new(repository),
            autotune,
        ))
    }

    /// Activate the elastic ingestion pipeline with SQLite persistence.
    ///
    /// Resolves `ElasticConfig` from the provided options, then wires
    /// `RayonCpuPool` → `CpuBridge` → `SqliteVectorRepository` →
    /// `ResourceDownloader` → `ElasticIngestion`. Only available under the
    /// `persistence` feature.
    ///
    /// # Errors
    ///
    /// Returns an error if the Rayon pool or SQLite pool fails to initialize.
    #[cfg(feature = "persistence")]
    pub async fn with_elastic(
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

        // 3. SQLite pool → repository (WAL mode, auto-creates parent dir)
        let pool = sqlite_persistence::create_pool(&config.db_path, config.db_pool_size)?;
        sqlite_persistence::setup_schema(&pool).await?;
        let repository: DynVectorRepository = Arc::new(SqliteVectorRepository::new(pool));

        let ingestion = Self::build_elastic(repository, &config)?;
        self.elastic_ingestion = Some(Arc::new(ingestion));
        Ok(self)
    }

    /// Activate the elastic ingestion pipeline with the dependency-free
    /// `StreamRepository` JSONL sink (no SQLite). Available in every build; use
    /// this for RAG vector export via `--output-vectors`.
    ///
    /// # Errors
    ///
    /// Returns an error if the output path cannot be opened for writing.
    pub fn with_stream(
        mut self,
        opts: &CrawlOptions,
        path: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let overrides = crate::infrastructure::autotuning::ElasticOverrides {
            cpu_cores: opts.elastic.cpu_cores,
            ram_budget_bytes: opts.elastic.ram_budget_bytes,
            max_resource_bytes: opts.elastic.max_resource_bytes,
            db_path: None,
        };
        let config = ElasticConfig::resolve(&overrides);
        let repository: DynVectorRepository =
            Arc::new(crate::infrastructure::stream::StreamRepository::new(path)?);

        let ingestion = Self::build_elastic(repository, &config)?;
        self.elastic_ingestion = Some(Arc::new(ingestion));
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CrawlerConfig;
    use crate::infrastructure::config::ScraperConfig;
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
}
