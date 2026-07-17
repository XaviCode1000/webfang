//! MCP Server State — shared state with per-category backpressure
//!
//! McpState embeds the application Container for DI and adds
//! tokio::sync::Semaphore instances to limit concurrent operations
//! per tool category, protecting the 8GB RAM / HDD hardware.

use std::sync::Arc;
use tokio::sync::Semaphore;

use webfang_core::adapters::downloader::Downloader;
use webfang_core::di::Container;
use webfang_core::domain::DomInspectorPort;

/// Per-category semaphore limits for backpressure.
/// Tuned for Intel i5-4590 (4C), 8GB DDR3, HDD.
#[derive(Debug)]
pub struct CategoryLimits {
    /// AI inference tools (tract-onnx, spawn_blocking heavy)
    pub ai: usize,
    /// HTTP scraping tools (network I/O, WAF checks)
    pub scraping: usize,
    /// Export tools (file I/O, serialization)
    pub export: usize,
    /// Obsidian vault tools (disk scan, embeddings)
    pub obsidian: usize,
    /// Content processing tools (CPU-bound HTML parsing)
    pub content: usize,
    /// URL utility tools (lightweight, string ops)
    pub url_utils: usize,
    /// Security tools (WAF detection, metrics)
    pub security: usize,
    /// Asset download tools (file I/O, network)
    pub assets: usize,
}

impl Default for CategoryLimits {
    fn default() -> Self {
        Self {
            ai: 2,         // Heavy CPU inference — limit strictly
            scraping: 8,   // Network I/O — can handle more concurrent
            export: 4,     // File I/O — moderate limit for HDD
            obsidian: 3,   // Disk scan + embeddings — protect vault I/O
            content: 6,    // CPU-bound HTML parsing — moderate
            url_utils: 16, // Lightweight string ops — high limit
            security: 8,   // WAF detection — moderate
            assets: 4,     // File downloads — protect HDD
        }
    }
}

/// Shared state for the MCP server.
///
/// Embeds the Container for dependency injection and provides
/// per-category semaphores for backpressure control.
#[derive(Clone)]
pub struct McpState {
    /// Application DI container (single source of truth)
    pub container: Arc<Container>,
    /// Per-category concurrency limits
    pub limits: Arc<CategoryLimits>,
    /// Semaphores for each category
    pub semaphores: Arc<CategorySemaphores>,
    /// Shared Downloader for connection pooling across MCP tool calls
    pub downloader: Option<Arc<Downloader>>,
    /// DOM inspector for CSS selector diagnostics (None = no diagnostics)
    pub inspector: Option<Arc<dyn DomInspectorPort>>,
}

/// Semaphore instances for each tool category.
#[derive(Debug)]
pub struct CategorySemaphores {
    pub ai: Arc<Semaphore>,
    pub scraping: Arc<Semaphore>,
    pub export: Arc<Semaphore>,
    pub obsidian: Arc<Semaphore>,
    pub content: Arc<Semaphore>,
    pub url_utils: Arc<Semaphore>,
    pub security: Arc<Semaphore>,
    pub assets: Arc<Semaphore>,
}

impl McpState {
    /// Create a new McpState with the given container and default limits.
    pub fn new(container: Container) -> Self {
        let limits = Arc::new(CategoryLimits::default());
        let semaphores = Arc::new(CategorySemaphores::from_limits(&limits));
        Self {
            container: Arc::new(container),
            limits,
            semaphores,
            downloader: None,
            inspector: None,
        }
    }

    /// Create with custom category limits.
    pub fn with_limits(container: Container, limits: CategoryLimits) -> Self {
        let limits = Arc::new(limits);
        let semaphores = Arc::new(CategorySemaphores::from_limits(&limits));
        Self {
            container: Arc::new(container),
            limits,
            semaphores,
            downloader: None,
            inspector: None,
        }
    }

    /// Set a shared Downloader for connection pooling across tool calls.
    #[must_use]
    pub fn with_downloader(mut self, downloader: Arc<Downloader>) -> Self {
        self.downloader = Some(downloader);
        self
    }

    /// Set a DOM inspector for CSS selector diagnostics.
    ///
    /// When set, failed selector extractions produce a `SelectorDiagnostic`
    /// with DOM structure analysis and closest-match suggestions.
    /// When `None` (default), diagnostics are `null` in the response.
    #[must_use]
    pub fn with_inspector(mut self, inspector: Arc<dyn DomInspectorPort>) -> Self {
        self.inspector = Some(inspector);
        self
    }
}

impl CategorySemaphores {
    pub fn from_limits(limits: &CategoryLimits) -> Self {
        // Clamp to >= 1 to prevent deadlock from zero-permit semaphores
        let clamp = |v: usize| v.max(1);
        Self {
            ai: Arc::new(Semaphore::new(clamp(limits.ai))),
            scraping: Arc::new(Semaphore::new(clamp(limits.scraping))),
            export: Arc::new(Semaphore::new(clamp(limits.export))),
            obsidian: Arc::new(Semaphore::new(clamp(limits.obsidian))),
            content: Arc::new(Semaphore::new(clamp(limits.content))),
            url_utils: Arc::new(Semaphore::new(clamp(limits.url_utils))),
            security: Arc::new(Semaphore::new(clamp(limits.security))),
            assets: Arc::new(Semaphore::new(clamp(limits.assets))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use webfang_core::domain::CrawlerConfig;
    use webfang_core::infrastructure::config::ScraperConfig;
    use webfang_core::infrastructure::scraper::dom_inspector::NoOpInspector;

    /// Build a minimal Container for testing (async, needs a temp output dir).
    async fn test_container() -> (TempDir, Container) {
        let tmp = TempDir::new().expect("create temp dir");
        let crawler_config = CrawlerConfig::new(url::Url::parse("https://example.com").unwrap());
        let scraper_config = ScraperConfig {
            output_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let container = Container::new(crawler_config, scraper_config)
            .await
            .expect("create test container");
        (tmp, container)
    }

    #[test]
    fn test_default_limits_are_reasonable() {
        let limits = CategoryLimits::default();
        assert!(limits.ai >= 1, "AI limit must allow at least 1 concurrent");
        assert!(limits.scraping >= 1, "Scraping limit must allow at least 1");
        assert!(
            limits.ai < limits.scraping,
            "AI should be more restricted than scraping"
        );
    }

    #[test]
    fn test_semaphores_created_with_correct_permits() {
        let limits = CategoryLimits::default();
        let semaphores = CategorySemaphores::from_limits(&limits);
        assert_eq!(semaphores.ai.available_permits(), limits.ai);
        assert_eq!(semaphores.scraping.available_permits(), limits.scraping);
        assert_eq!(semaphores.obsidian.available_permits(), limits.obsidian);
    }

    #[tokio::test]
    async fn test_new_state_has_no_inspector() {
        let (_tmp, container) = test_container().await;
        let state = McpState::new(container);
        assert!(state.inspector.is_none(), "inspector must default to None");
    }

    #[tokio::test]
    async fn test_with_inspector_sets_inspector() {
        let (_tmp, container) = test_container().await;
        let inspector: Arc<dyn DomInspectorPort> = Arc::new(NoOpInspector);
        let state = McpState::new(container).with_inspector(inspector);
        assert!(
            state.inspector.is_some(),
            "inspector must be set after with_inspector"
        );
    }

    #[tokio::test]
    async fn test_with_limits_has_no_inspector() {
        let (_tmp, container) = test_container().await;
        let state = McpState::with_limits(container, CategoryLimits::default());
        assert!(
            state.inspector.is_none(),
            "inspector must default to None in with_limits"
        );
    }
}
