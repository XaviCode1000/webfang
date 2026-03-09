//! Domain layer — Core business entities (puro, sin frameworks)
//!
//! Following Clean Architecture: no dependencies on infrastructure.
//! This layer contains the business logic that doesn't depend on external frameworks.

pub mod crawler_entities;
pub mod entities;
pub mod exporter;
pub mod value_objects;

pub use crawler_entities::{
    matches_pattern, ContentType, CrawlError, CrawlResult, CrawlerConfig, CrawlerConfigBuilder,
    DiscoveredUrl,
};
pub use entities::{DocumentChunk, DownloadedAsset, ExportFormat, ExportState, ScrapedContent};
pub use exporter::{ExportResult, Exporter, ExporterConfig, ExporterError};
pub use value_objects::ValidUrl;
