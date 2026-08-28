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

pub mod axtree_port;
pub mod clock;
pub mod config;
pub mod config_value;
pub mod cpu_executor;
pub mod crawl_job;
pub mod crawler_entities;
pub mod credentials;
pub mod dom_inspector;
pub mod entities;
pub mod error;
pub mod exporter;
pub mod extraction_quality;
pub mod fingerprint_repository;
pub mod html_cleaner;
pub mod http_config;
pub mod http_error;
pub mod http_port;
pub mod js_renderer;
pub mod js_strategy;
pub mod link_extractor;
pub mod llm;
/// Typed 8-state page lifecycle (persisted enum + compile-time
/// typestate wrapper). See module docs for the legacy-encoding mapping.
pub mod page_state;
pub mod pattern_matching;
/// Pipeline stage definitions and scraped item types for the crawl pipeline.
pub mod pipeline_item;
pub mod ports;
pub mod profile;
pub mod repositories;
pub mod repository;
pub mod result;
pub mod scraper_port;
pub mod session_port;
pub mod site;
pub mod url_validation;
pub mod url_validator;
pub mod user_agent;
pub mod value_objects;
pub mod waf;

/// Downloader port — domain-owned trait for page fetching (ADR-0010).
pub mod downloader_port;

/// Crawler port — SitemapConfig + pure helpers (ADR-0010).
pub mod crawler_port;

/// Budget model: Global→Domain→Operation→Asset concurrency tiers,
/// canonical clamp, hardware-detector seam, and pure derivation fns.
pub mod budget;
pub mod content_processor;
pub mod embedding_port;
/// Shared excerpt byline-repair invariant (regex + whitespace), pure & IO-free.
pub(crate) mod excerpt_repair;
pub mod llm_port;
pub mod note_repository;
/// Single source of truth for user-facing options (ADR-002): declarative
/// specs that generate clap args, JSON Schema, and shared validators.
pub mod options_spec;
/// Persistence mode — unified control-plane for `--resume`/`--state-dir` and
/// `--checkpoint-interval`/`--no-checkpoint` (domain pure, no IO).
pub mod persistence;
pub mod semantic_cleaner;
pub mod semantic_inspector;
pub mod text_chunker;

// Re-exports for backward compatibility (crate::domain::X)
pub use clock::{Clock, MockClock, MockUtcClock, SystemClock, SystemUtcClock, UtcClock};
pub use config::{
    AssetNamingStrategy, AutotuningConfig, ConcurrencyConfig, ElasticOverrides, ExportFormat,
    OutputFormat, PipelineOutputFormat, ScraperConfig,
};
pub use content_processor::ContentProcessor;
pub use crawl_job::{ContentType, DiscoveredUrl};
pub use credentials::{AccessToken, ApiKey, CredentialStore, SecretCredential, SensitiveString};
pub use dom_inspector::{
    DomInspectorPort, DomStructureReport, ExtractResult, RepairFailureDiagnostic,
    SelectorDiagnostic, SelectorErrorKind, SelectorSuggestion,
};
pub use embedding_port::EmbeddingPort;
pub use note_repository::{
    IndexedNoteMeta, NoteChunkVector, NoteRepository, VaultNote, VaultNoteReader,
};
pub use semantic_inspector::{
    BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};
pub use text_chunker::TextChunker;

pub use axtree_port::AxTreePort;
pub use cpu_executor::CpuExecutorPort;
pub use entities::{
    DocumentChunk, DocumentChunkExported, DocumentChunkUnvalidated, DocumentChunkValidated,
    DownloadedAsset, Draft, ExportState, Exported, ScrapedContent, Validated, ValidationError,
};
pub use error::{CrawlError, CrawlErrorCategory, DomainError};
pub use exporter::{ExportResult, Exporter, ExporterConfig};
pub use html_cleaner::clean_html;
pub use http_config::{HttpClientConfig, UnknownProfileError};
pub use http_error::{HttpError, HttpResult};
pub use http_port::{HttpClientPort, HttpResponse};
pub use js_renderer::{JsRenderError, JsRenderer};
pub use js_strategy::JsStrategy;
pub use link_extractor::{LinkExtractor, LinkProcessor};
pub use llm::validation::{validate_record, validate_schema, SchemaError};
pub use pattern_matching::{match_url_pattern, matches_pattern};
pub use pipeline_item::{FilterReason, PipelineStage, RejectReason, ScrapedItem, StageOutcome};
pub use ports::AssetDownloaderPort;
pub use profile::{profile_from_name, valid_profile_names};
pub use repositories::CrawlResultRepository;
pub use repository::VectorRepository;
pub use result::CrawlResult;
pub use scraper_port::ScraperPort;
pub use session_port::{SessionId, SessionPoolConfig, SessionPort};
pub use site::{CrawlerConfig, CrawlerConfigBuilder};
pub use url_validator::{StaticUrlValidator, UrlValidator, UrlValidatorTrait};
pub use user_agent::{UserAgentPool, UserAgentProvider};
pub use value_objects::{CorrelationId, ValidUrl};
pub use waf::{
    EvidenceSource, InspectionContext, WafEvidence, WafInspectorPort, WafTier, WafVerdict,
};

/// Compression types supported for sitemap parsing
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompressionType {
    /// No compression — raw, uncompressed content.
    None,
    /// gzip (RFC 1952) — common in HTTP Content-Encoding.
    Gzip,
    /// DEFLATE (RFC 1951) — raw deflate without zlib wrapper.
    Deflate,
    /// Brotli — modern high-ratio compression, used by CDNs.
    Brotli,
    /// Zstandard — modern compression balancing speed and ratio.
    Zstd,
}

/// Batch of URLs for paginated processing
#[derive(Debug, Clone)]
pub struct UrlBatch {
    /// URLs in this batch.
    pub urls: Vec<Url>,
    /// Zero-based batch index for ordering and progress tracking.
    pub batch_id: u32,
    /// `true` when additional batches follow; `false` for the final batch.
    pub has_more: bool,
}

/// Result of URL validation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationResult {
    /// URL is valid and allowed.
    Valid,
    /// URL is invalid; `String` describes the reason.
    Invalid(String),
    /// URL is valid but must be redirected to the contained canonical URL.
    NeedsRedirect(Url),
}
