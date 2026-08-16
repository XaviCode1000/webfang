//! MCP Server State — shared state with per-category backpressure
//!
//! McpState embeds the application Container for DI and adds
//! tokio::sync::Semaphore instances to limit concurrent operations
//! per tool category, protecting the 8GB RAM / HDD hardware.

use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::mcp_server::metrics::{MetricsSnapshot, ScrapeEvent, ScrapeMetrics};
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
    /// Process-lifetime scrape metrics, shared across all per-session clones
    /// (REQ-06). Locked only in short synchronous sections (REQ-07).
    pub metrics: Arc<Mutex<ScrapeMetrics>>,
    /// Cancellation token for graceful shutdown propagation.
    pub cancel_token: CancellationToken,
}

/// Semaphore instances for each tool category.
#[derive(Debug)]
pub struct CategorySemaphores {
    /// Semaphore limiting concurrent AI tool calls
    pub ai: Arc<Semaphore>,
    /// Semaphore limiting concurrent scraping tool calls
    pub scraping: Arc<Semaphore>,
    /// Semaphore limiting concurrent export tool calls
    pub export: Arc<Semaphore>,
    /// Semaphore limiting concurrent Obsidian tool calls
    pub obsidian: Arc<Semaphore>,
    /// Semaphore limiting concurrent content processing tool calls
    pub content: Arc<Semaphore>,
    /// Semaphore limiting concurrent URL utility tool calls
    pub url_utils: Arc<Semaphore>,
    /// Semaphore limiting concurrent security & diagnostics tool calls
    pub security: Arc<Semaphore>,
    /// Semaphore limiting concurrent asset download tool calls
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
            metrics: Arc::new(Mutex::new(ScrapeMetrics::default())),
            cancel_token: CancellationToken::new(),
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
            metrics: Arc::new(Mutex::new(ScrapeMetrics::default())),
            cancel_token: CancellationToken::new(),
        }
    }

    /// Trigger graceful shutdown by cancelling the token.
    ///
    /// All clones of this state share the same token (via `Arc` interior
    /// mutability), so a single call propagates to all holders.
    pub fn shutdown_signal(&self) {
        self.cancel_token.cancel();
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

    /// Record a scrape event into the shared accumulator.
    ///
    /// REQ-07: short synchronous critical section — the lock is acquired and
    /// dropped entirely within this call (the tracing event emitted inside
    /// `record` is synchronous), so it is never held across an `.await`.
    /// REQ-10: a poisoned mutex is recovered via `into_inner`, never a panic.
    pub fn record_scrape(&self, event: ScrapeEvent) {
        self.metrics
            .lock()
            // LCOV_EXCL_LINE defensive: lock-poisoning — poison is recovered via into_inner, never a panic
            .unwrap_or_else(|e| e.into_inner())
            .record(event);
    }

    /// Record a scrape event stamped with the tool call's run-root identity.
    ///
    /// The sole call-side helper for identified scrape metrics (#698): it owns
    /// the [`ScrapeEvent::identified`] construction so each MCP tool handler is
    /// a one-line record call. `start` is the operation's start instant and is
    /// converted to an elapsed [`Duration`](std::time::Duration) here, keeping
    /// timing bookkeeping in one place.
    pub fn record_scrape_identity(
        &self,
        tool: &'static str,
        domain: String,
        outcome: crate::mcp_server::metrics::Outcome,
        count: usize,
        start: std::time::Instant,
        correlation: &webfang_core::domain::CorrelationId,
    ) {
        let event =
            ScrapeEvent::identified(tool, domain, outcome, count, start.elapsed(), correlation);
        self.record_scrape(event);
    }

    /// Produce a point-in-time metrics snapshot from the shared accumulator.
    ///
    /// REQ-07: lock → clone snapshot → release. REQ-10: poison recovery, never
    /// a panic.
    #[must_use]
    pub fn metrics_snapshot(&self) -> MetricsSnapshot {
        self.metrics
            .lock()
            // LCOV_EXCL_LINE defensive: lock-poisoning — poison is recovered via into_inner, never a panic
            .unwrap_or_else(|e| e.into_inner())
            .snapshot()
    }
}

impl CategorySemaphores {
    /// Build semaphores from per-category concurrency limits.
    ///
    /// Each permit count is clamped to a minimum of one so that a zero-permit
    /// limit cannot deadlock concurrent tool calls.
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

    /// REQ-06: the metrics accumulator is shared across per-session clones
    /// (`server.rs` clones `McpState` per session). Mirrors the `Arc::ptr_eq`
    /// pattern from `server.rs::test_mcp_state_with_downloader`.
    #[tokio::test]
    async fn metrics_shared_across_clones() {
        use crate::mcp_server::metrics::Outcome;
        use std::time::Duration;

        let (_tmp, container) = test_container().await;
        let state = McpState::new(container);
        let state2 = state.clone();

        assert!(
            Arc::ptr_eq(&state.metrics, &state2.metrics),
            "cloned McpState must share the same metrics Arc"
        );

        // A scrape recorded through one clone is visible from the other.
        state.record_scrape(ScrapeEvent::new(
            "scrape_url",
            "a.com".to_string(),
            Outcome::Success,
            2,
            Duration::from_millis(10),
        ));

        let snap = state2.metrics_snapshot();
        assert_eq!(snap.total_events, 1, "record-through-clone is visible");
        assert_eq!(snap.success_count, 1, "success bucket recorded");
    }
}
