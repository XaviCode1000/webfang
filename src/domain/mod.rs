//! Domain layer — Core business entities (puro, sin frameworks)
//!
//! Following Clean Architecture: no dependencies on infrastructure.
//! This layer contains the business logic that doesn't depend on external frameworks.
//!
//! # Module Structure
//!
//! - [`site`] — Site configuration (`CrawlerConfig`, `CrawlerConfigBuilder`)
//! - [`crawl_job`] — Crawl entities (`DiscoveredUrl`, `ContentType`)
//! - [`result`] — Crawl results (`CrawlResult`)
//! - [`error`] — Error types (`CrawlError`)
//! - [`pattern_matching`] — SSRF-safe URL pattern matching

use url::Url;

pub mod crawl_job;
pub mod crawler_entities;
pub mod entities;
pub mod error;
pub mod exporter;
pub mod js_renderer;
pub mod pattern_matching;
pub mod result;
pub mod site;
pub mod value_objects;

#[cfg(feature = "ai")]
pub mod semantic_cleaner;

// Re-exports for backward compatibility (crate::domain::X)
pub use crawl_job::{ContentType, DiscoveredUrl};

pub use entities::{DocumentChunk, DownloadedAsset, ExportFormat, ExportState, ScrapedContent};
pub use error::CrawlError;
pub use exporter::{ExportResult, Exporter, ExporterConfig, ExporterError};
pub use js_renderer::{JsRenderError, JsRenderer};
pub use pattern_matching::matches_pattern;
pub use result::CrawlResult;
pub use site::{CrawlerConfig, CrawlerConfigBuilder};
pub use value_objects::ValidUrl;

/// Compression types supported for sitemap parsing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressionType {
    None,
    Gzip,
    Deflate,
    Brotli,
    Zstd,
}

/// Batch of URLs for paginated processing
#[derive(Debug, Clone)]
pub struct UrlBatch {
    pub urls: Vec<Url>,
    pub batch_id: u32,
    pub has_more: bool,
}

/// Result of URL validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    Valid,
    Invalid(String), // reason
    NeedsRedirect(Url), // new URL
}
