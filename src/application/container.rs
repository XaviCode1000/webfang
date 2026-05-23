//! Dependency Injection Container
//!
//! Provides a centralized way to wire up all services and their dependencies.
//! Following Clean Architecture, the container lives in the application layer
//! and creates instances of infrastructure implementations.

use std::sync::Arc;

use crate::application::crawl_result_repository::CrawlResultRepositoryImpl;
use crate::application::http_client::{HttpClient, HttpClientConfig};
use crate::domain::{repositories::CrawlResultRepository, CrawlerConfig};
use crate::infrastructure::config::ScraperConfig;
use crate::infrastructure::export::state_store::StateStore;

/// Dependency Injection Container
///
/// Holds all service instances and their configurations.
/// Services are created once and reused throughout the application.
#[derive(Clone)]
pub struct Container {
    pub crawler_config: CrawlerConfig,
    pub scraper_config: ScraperConfig,
    pub http_client: Arc<HttpClient>,
    pub state_store: Option<Arc<StateStore>>,
    pub crawl_result_repo: Option<Arc<dyn CrawlResultRepository>>,
}

impl Container {
    /// Create a new container with the given configurations.
    ///
    /// # Arguments
    ///
    /// * `crawler_config` - Configuration for crawling behavior
    /// * `scraper_config` - Configuration for scraping behavior
    ///
    /// # Returns
    ///
    /// A new container instance with all services initialized
    pub async fn new(
        crawler_config: CrawlerConfig,
        scraper_config: ScraperConfig,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // Initialize infrastructure services
        let http_client = Arc::new(HttpClient::new(HttpClientConfig::default())?);

        // State store is optional (for resume mode)
        let state_store = None;

        // Crawl result repository using append-only log
        let log_path = scraper_config.output_dir.join("crawl_results.bin");
        let crawl_result_repo = match CrawlResultRepositoryImpl::new(log_path, 1024) {
            Ok(repo) => Some(Arc::new(repo) as Arc<dyn CrawlResultRepository>),
            Err(e) => {
                tracing::warn!("no se pudo inicializar el repositorio: {e}");
                None
            },
        };

        Ok(Self {
            crawler_config,
            scraper_config,
            http_client,
            state_store,
            crawl_result_repo,
        })
    }

    /// Set the state store for resume functionality.
    pub fn with_state_store(mut self, state_store: StateStore) -> Self {
        self.state_store = Some(Arc::new(state_store));
        self
    }

    /// Get a repository for crawl results (backed by append-only log).
    pub fn crawl_result_repository(&self) -> Option<Arc<dyn CrawlResultRepository>> {
        self.crawl_result_repo.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CrawlerConfig;
    use crate::infrastructure::config::ScraperConfig;
    use tempfile::TempDir;

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_container_wires_crawl_result_repository() {
        let tmp = TempDir::new().unwrap();
        let crawler_config = CrawlerConfig::new(url::Url::parse("https://example.com").unwrap());
        let scraper_config = ScraperConfig {
            output_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };

        let container = Container::new(crawler_config, scraper_config)
            .await
            .unwrap();
        let repo = container.crawl_result_repository();
        assert!(
            repo.is_some(),
            "crawl_result_repository() debe retornar Some"
        );
    }
}
