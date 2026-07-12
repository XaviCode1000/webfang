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

pub mod config;
pub mod crawl_job;
pub mod crawler_entities;
pub mod credentials;
pub mod entities;
pub mod error;
pub mod exporter;
pub mod js_renderer;
pub mod js_strategy;
pub mod link_extractor;
pub mod pattern_matching;
pub mod pipeline_item;
pub mod repositories;
pub mod repository;
pub mod result;
pub mod site;
pub mod url_validation;
pub mod url_validator;
pub mod value_objects;

#[cfg(feature = "ai")]
pub mod semantic_cleaner;

// Re-exports for backward compatibility (crate::domain::X)
pub use config::{ConcurrencyConfig, ExportFormat, OutputFormat, PipelineOutputFormat};
pub use crawl_job::{ContentType, DiscoveredUrl};
pub use credentials::{
    AccessToken, ApiKey, CredentialError, CredentialStore, SecretCredential, SensitiveString,
};

pub use entities::{
    DocumentChunk, DocumentChunkExported, DocumentChunkUnvalidated, DocumentChunkValidated,
    DownloadedAsset, Draft, ExportState, Exported, ScrapedContent, Validated, ValidationError,
};
pub use error::CrawlError;
pub use exporter::{ExportResult, Exporter, ExporterConfig, ExporterError};
pub use js_renderer::{JsRenderError, JsRenderer};
pub use js_strategy::JsStrategy;
pub use link_extractor::{LinkExtractor, LinkProcessor};
pub use pattern_matching::matches_pattern;
pub use pipeline_item::{PipelineStage, ScrapedItem, StageOutcome};
pub use repositories::CrawlResultRepository;
pub use repository::VectorRepository;
pub use result::CrawlResult;
pub use site::{CrawlerConfig, CrawlerConfigBuilder};
pub use url_validator::UrlValidator;
pub use value_objects::{CorrelationId, ValidUrl};

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
    Invalid(String),    // reason
    NeedsRedirect(Url), // new URL
}
