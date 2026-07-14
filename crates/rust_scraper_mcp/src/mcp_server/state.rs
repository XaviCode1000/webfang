//! MCP Server State — shared state with per-category backpressure
//!
//! McpState embeds the application Container for DI and adds
//! tokio::sync::Semaphore instances to limit concurrent operations
//! per tool category, protecting the 8GB RAM / HDD hardware.

use std::sync::Arc;
use tokio::sync::Semaphore;

use rust_scraper_core::adapters::downloader::Downloader;
use rust_scraper_core::di::Container;

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
        }
    }

    /// Set a shared Downloader for connection pooling across tool calls.
    #[must_use]
    pub fn with_downloader(mut self, downloader: Arc<Downloader>) -> Self {
        self.downloader = Some(downloader);
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
}
