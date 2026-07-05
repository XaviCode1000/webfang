//! Domain entities — Core business types
//!
//! These are the fundamental data structures used throughout the application.
//! They are serializable for persistence but contain no business logic.

use std::collections::HashMap;
use std::marker::PhantomData;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{CorrelationId, ValidUrl};

// ============================================================================
// Validation Errors
// ============================================================================

/// Errors that can occur during DocumentChunk validation
///
/// Domain errors define WHAT failed, not HOW to present to users.
/// Error messages are neutral identifiers for programmatic handling.
/// Presentation layer (CLI/TUI) maps these to user-friendly messages.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("empty_content")]
    EmptyContent,

    #[error("empty_title")]
    EmptyTitle,

    #[error("invalid_url: {0}")]
    InvalidUrl(String),

    #[error("invalid_metadata: {0}")]
    InvalidMetadata(String),
}

// ============================================================================
// Typestate Markers — Private state types for DocumentChunk
// ============================================================================

/// Private state marker: DocumentChunk is newly created, not validated
/// No public constructor - only created via From<ScrapedContent>
#[derive(Clone, Copy)]
pub struct Draft;

/// Private state marker: DocumentChunk has passed validation checks
/// Can be exported to disk
#[derive(Clone, Copy)]
pub struct Validated;

/// Private state marker: DocumentChunk has been exported
/// Metadata includes export path
#[derive(Clone, Copy)]
pub struct Exported;

/// Constructor for DocumentChunk<Draft> that bypasses From<ScrapedContent> (tests only)
#[cfg(test)]
impl DocumentChunk<Draft> {
    pub fn test_new(
        id: Uuid,
        url: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            title: title.into(),
            content: content.into(),
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: None,
            _state: PhantomData,
        }
    }
}

/// Public constructor for DocumentChunk<Draft> in production code
/// Required for modules that create DocumentChunk directly (e.g., chunker, relevance_scorer)
impl DocumentChunk<Draft> {
    pub fn new(
        id: Uuid,
        url: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            title: title.into(),
            content: content.into(),
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: None,
            _state: PhantomData,
        }
    }
}

/// Constructor with metadata for chunks that need to preserve source metadata
impl DocumentChunk<Draft> {
    pub fn with_metadata(
        id: Uuid,
        url: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
        metadata: HashMap<String, String>,
    ) -> Self {
        Self {
            id,
            url: url.into(),
            title: title.into(),
            content: content.into(),
            metadata,
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: None,
            _state: PhantomData,
        }
    }
}

/// Represents a downloaded asset (image or document)
///
/// Contains metadata about the original URL and local storage location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DownloadedAsset {
    /// Original URL of the asset
    pub url: String,
    /// Local path where asset was saved
    pub local_path: String,
    /// Asset type: "image" or "document"
    pub asset_type: String,
    /// File size in bytes
    pub size: u64,
}

/// Represents scraped content from a web page
///
/// This is the main output type of the scraper, containing:
/// - Extracted content (title, text)
/// - Metadata (author, date, excerpt)
/// - Downloaded assets (images, documents)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrapedContent {
    /// Title of the page/article
    pub title: String,
    /// Main content extracted (clean, without ads/nav)
    pub content: String,
    /// Original URL (validated)
    pub url: ValidUrl,
    /// Excerpt/summary if available
    pub excerpt: Option<String>,
    /// Author if available
    pub author: Option<String>,
    /// Publication date if available (ISO 8601 format)
    pub date: Option<String>,
    /// The HTML source (optional, for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Downloaded assets (images, documents)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub assets: Vec<DownloadedAsset>,
    /// Distributed tracing correlation ID (links to OTel span context)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

impl std::fmt::Display for ScrapedContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let title = if self.title.is_empty() {
            "(untitled)"
        } else {
            self.title.as_str()
        };
        write!(f, "{title} — {}", self.url)
    }
}

/// Export format variants for RAG pipeline
///
/// Defines the supported output formats when exporting scraped content
/// for use in retrieval-augmented generation systems.
///
/// These formats are designed for RAG/embedding pipelines, NOT for
/// individual file output (see OutputFormat for that).
///
/// | Format | Extension | Use Case |
/// |--------|-----------|----------|
/// | Jsonl | .jsonl | One JSON object per line, optimal for RAG |
/// | Auto | .auto | Auto-detect from existing files |
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum, Default,
)]
pub enum ExportFormat {
    /// JSONL format (JSON Lines - one JSON object per line)
    /// Optimal for RAG pipelines and vector database ingestion
    #[default]
    Jsonl,
    /// Vector format (JSON with metadata header)
    /// Supports embeddings and cosine similarity
    Vector,
    /// Auto-detect format from existing export files
    Auto,
}

impl ExportFormat {
    /// Parse from string (case-insensitive).
    /// Note: Named `parse_str` to avoid confusion with `FromStr::from_str`.
    pub fn parse_str(s: &str) -> Result<Self, &'static str> {
        match s.to_lowercase().as_str() {
            "jsonl" => Ok(ExportFormat::Jsonl),
            "vector" => Ok(ExportFormat::Vector),
            "auto" => Ok(ExportFormat::Auto),
            _ => Err("Invalid export format. Use 'jsonl', 'vector', or 'auto'"),
        }
    }
    /// Returns the file extension for this format
    #[must_use]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Vector => "json",
            Self::Auto => "auto",
        }
    }

    /// Returns a human-readable name for this format
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Jsonl => "JSONL",
            Self::Vector => "Vector",
            Self::Auto => "Auto",
        }
    }
}

/// A document chunk ready for export to RAG pipelines
///
/// Represents a single unit of content that can be:
/// - Embedded in a vector database
/// - Exported in various formats (JSONL, Markdown)
///
/// The embeddings field is optional because in the initial scraping phase
/// the AI model may not be available yet. It can be populated later
/// by a separate embedding pipeline.
///
/// Uses typestate pattern: must call `.validate()` before export.
///
/// ```compile_fail
/// use rust_scraper::domain::DocumentChunkUnvalidated;
/// use rust_scraper::domain::Exporter;
/// use rust_scraper::domain::exporter::ExporterConfig;
/// use rust_scraper::infrastructure::export::FileExporter;
/// use rust_scraper::ExportFormat;
/// use uuid::Uuid;
/// use std::path::PathBuf;
///
/// // DocumentChunk<Draft> cannot be compiled into the export path.
/// // The typestate pattern enforces .validate() before export at compile time.
/// let config = ExporterConfig::new(PathBuf::from("/tmp"), ExportFormat::Jsonl, "test");
/// let exporter = FileExporter::new(config);
/// let chunk = DocumentChunkUnvalidated::new(
///     Uuid::new_v4(),
///     "https://example.com",
///     "Test Title",
///     "Test content",
/// );
///
/// // This MUST fail: Exporter::export() expects DocumentChunk<Validated>
/// let _ = exporter.export(chunk);
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk<S = Draft> {
    /// Unique identifier for this chunk (UUID v4)
    pub id: Uuid,
    /// Source URL where this content was scraped from
    pub url: String,
    /// Title of the source page/article
    pub title: String,
    /// The actual text content (cleaned, ready for embedding)
    pub content: String,
    /// Additional metadata extracted during scraping
    /// Keys: author, date, excerpt, domain, etc.
    pub metadata: HashMap<String, String>,
    /// Timestamp when this content was scraped (UTC)
    pub timestamp: DateTime<Utc>,
    /// Optional embedding vector (for vector database storage)
    /// Populated by embedding pipeline after initial scrape
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeddings: Option<Vec<f32>>,
    /// W3C TraceContext correlation ID for distributed tracing
    /// Optional - when present, enables request correlation across services
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    /// Typestate marker (crate-visibile for tests)
    #[serde(skip)]
    pub(crate) _state: PhantomData<S>,
}

/// Alias for backward compatibility - DocumentChunk in Draft state
/// Use DocumentChunk<Draft> for new code, DocumentChunk for existing code
pub type DocumentChunkUnvalidated = DocumentChunk<Draft>;
pub type DocumentChunkValidated = DocumentChunk<Validated>;
pub type DocumentChunkExported = DocumentChunk<Exported>;

// NOTE: DocumentChunk (non-generic) is re-exported from domain/mod.rs for backward compatibility

/// Conversion from ScrapedContent creates DocumentChunk in Draft state
impl From<ScrapedContent> for DocumentChunk<Draft> {
    fn from(scraped: ScrapedContent) -> Self {
        let mut metadata = HashMap::new();

        if let Some(excerpt) = scraped.excerpt {
            metadata.insert("excerpt".to_string(), excerpt);
        }
        if let Some(author) = scraped.author {
            metadata.insert("author".to_string(), author);
        }
        if let Some(date) = scraped.date {
            metadata.insert("date".to_string(), date);
        }

        if let Ok(url) = url::Url::parse(&scraped.url.to_string()) {
            if let Some(host) = url.host_str() {
                metadata.insert("domain".to_string(), host.to_string());
            }
        }

        Self {
            id: Uuid::new_v4(),
            url: scraped.url.to_string(),
            title: scraped.title,
            content: scraped.content,
            metadata,
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: scraped.correlation_id.map(|c| c.to_traceparent()),
            _state: PhantomData,
        }
    }
}

impl DocumentChunk<Draft> {
    /// Create a new DocumentChunk from ScrapedContent (Draft state)
    ///
    /// This is the main conversion method from the scraper's output
    /// to the RAG pipeline's input format.
    #[must_use]
    pub fn from_scraped_content(scraped: &ScrapedContent) -> Self {
        Self::from_scraped_content_with_correlation(scraped, scraped.correlation_id.as_ref())
    }

    /// Create a new DocumentChunk from ScrapedContent with correlation ID
    ///
    /// Includes W3C traceparent for distributed tracing.
    #[must_use]
    pub fn from_scraped_content_with_correlation(
        scraped: &ScrapedContent,
        correlation_id: Option<&CorrelationId>,
    ) -> Self {
        let mut metadata = HashMap::new();

        // Extract optional fields into metadata HashMap
        if let Some(ref excerpt) = scraped.excerpt {
            metadata.insert("excerpt".to_string(), excerpt.clone());
        }
        if let Some(ref author) = scraped.author {
            metadata.insert("author".to_string(), author.clone());
        }
        if let Some(ref date) = scraped.date {
            metadata.insert("date".to_string(), date.clone());
        }

        // Extract domain from URL
        if let Ok(url) = url::Url::parse(&scraped.url.to_string()) {
            if let Some(host) = url.host_str() {
                metadata.insert("domain".to_string(), host.to_string());
            }
        }

        Self {
            id: Uuid::new_v4(),
            url: scraped.url.to_string(),
            title: scraped.title.clone(),
            content: scraped.content.clone(),
            metadata,
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: correlation_id.map(|c| c.to_traceparent()),
            _state: PhantomData,
        }
    }

    /// Create a new DocumentChunk with custom ID
    ///
    /// Use this when you need to preserve a specific ID (e.g., from a database)
    #[must_use]
    pub fn with_id(scraped: &ScrapedContent, id: Uuid) -> Self {
        let mut chunk = Self::from_scraped_content(scraped);
        chunk.id = id;
        chunk
    }

    /// Set embeddings for this chunk
    ///
    /// Typically called by the embedding pipeline after generating vectors
    #[must_use]
    pub fn with_embeddings(mut self, embeddings: Vec<f32>) -> Self {
        self.embeddings = Some(embeddings);
        self
    }

    /// Validate this Draft DocumentChunk
    ///
    /// Returns Validated state if content is valid:
    /// - content is not empty
    /// - title is not empty
    /// - URL is a valid HTTP/HTTPS URL
    /// - metadata values are reasonable
    ///
    /// # Errors
    /// Returns ValidationError if validation fails
    pub fn validate(self) -> Result<DocumentChunkValidated, ValidationError> {
        // Validation: content must not be empty
        if self.content.trim().is_empty() {
            return Err(ValidationError::EmptyContent);
        }

        // Validation: title must not be empty
        if self.title.trim().is_empty() {
            return Err(ValidationError::EmptyTitle);
        }

        // Validation: URL must be valid
        if let Err(e) = url::Url::parse(&self.url) {
            return Err(ValidationError::InvalidUrl(e.to_string()));
        }

        // Validation: metadata values should not be empty strings
        for (key, value) in &self.metadata {
            if value.trim().is_empty() {
                return Err(ValidationError::InvalidMetadata(format!(
                    "metadata key '{key}' has empty value"
                )));
            }
        }

        // Pure move: consume self (Draft state) and produce Validated state
        // No clones - all fields are moved from self to the new instance
        Ok(DocumentChunk {
            id: self.id,
            url: self.url,
            title: self.title,
            content: self.content,
            metadata: self.metadata,
            timestamp: self.timestamp,
            embeddings: self.embeddings,
            correlation_id: self.correlation_id,
            _state: PhantomData,
        })
    }
}

/// Methods available for any DocumentChunk state
impl<S> DocumentChunk<S> {
    /// Check if this chunk has embeddings
    #[must_use]
    pub fn has_embeddings(&self) -> bool {
        self.embeddings.is_some()
    }

    /// Get the text length (character count)
    #[must_use]
    pub fn text_length(&self) -> usize {
        self.content.len()
    }
}

/// Methods for Validated DocumentChunk
impl DocumentChunk<Validated> {
    /// Export this Validated DocumentChunk
    ///
    /// This method is only available for Validated state.
    /// Ensures content has passed validation before export.
    pub fn export(&self) -> &Self
    where
        Self: Send + Sync + 'static,
    {
        // Already validated, just return reference
        self
    }
}

/// Metadata for the export state file
///
/// Stored at ~/.cache/rust_scraper/state/<domain>.json
/// Tracks which URLs have been processed for a given domain
/// to support incremental exports and resume capability.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExportState {
    /// Domain this state belongs to (e.g., "example.com")
    pub domain: String,
    /// URLs that have been successfully exported
    pub processed_urls: Vec<String>,
    /// Last export timestamp
    pub last_export: Option<DateTime<Utc>>,
    /// Total documents exported
    pub total_exported: u64,
}

impl ExportState {
    /// Create a new ExportState for a domain
    #[must_use]
    pub fn new(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            processed_urls: Vec::new(),
            last_export: None,
            total_exported: 0,
        }
    }

    /// Mark a URL as processed
    pub fn mark_processed(&mut self, url: &str) {
        if !self.processed_urls.contains(&url.to_string()) {
            self.processed_urls.push(url.to_string());
            self.total_exported += 1;
        }
    }

    /// Check if a URL has been processed
    #[must_use]
    pub fn is_processed(&self, url: &str) -> bool {
        self.processed_urls.contains(&url.to_string())
    }

    /// Update last export timestamp
    pub fn update_timestamp(&mut self) {
        self.last_export = Some(Utc::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_downloaded_asset_creation() {
        let asset = DownloadedAsset {
            url: "https://example.com/image.png".to_string(),
            local_path: "/tmp/image.png".to_string(),
            asset_type: "image".to_string(),
            size: 1024,
        };

        assert_eq!(asset.url, "https://example.com/image.png");
        assert_eq!(asset.size, 1024);
    }

    #[test]
    fn test_scraped_content_with_minimal_fields() {
        let content = ScrapedContent {
            title: "Test Article".to_string(),
            content: "Test content".to_string(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };

        assert_eq!(content.title, "Test Article");
        assert!(content.excerpt.is_none());
        assert!(content.assets.is_empty());
    }

    #[test]
    fn test_scraped_content_with_all_fields() {
        let content = ScrapedContent {
            title: "Complete Article".to_string(),
            content: "Full content here".to_string(),
            url: ValidUrl::parse("https://example.com/article").unwrap(),
            excerpt: Some("A short excerpt".to_string()),
            author: Some("John Doe".to_string()),
            date: Some("2024-01-15".to_string()),
            html: Some("<html>test</html>".to_string()),
            assets: vec![DownloadedAsset {
                url: "https://example.com/img.png".to_string(),
                local_path: "/tmp/img.png".to_string(),
                asset_type: "image".to_string(),
                size: 2048,
            }],
            correlation_id: None,
        };

        assert_eq!(content.author, Some("John Doe".to_string()));
        assert_eq!(content.assets.len(), 1);
        assert_eq!(content.assets[0].size, 2048);
    }

    #[test]
    fn test_export_format_vector_extension() {
        assert_eq!(ExportFormat::Vector.extension(), "json");
    }

    #[test]
    fn test_export_format_vector_name() {
        assert_eq!(ExportFormat::Vector.name(), "Vector");
    }

    // -- ScrapedContent Display tests --

    fn make_scraped(title: &str, url: &str) -> ScrapedContent {
        ScrapedContent {
            title: title.to_string(),
            content: "body".to_string(),
            url: ValidUrl::parse(url).unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        }
    }

    #[test]
    fn scraped_content_display_shows_title_and_url() {
        let s = make_scraped("My Article", "https://example.com/post");
        let output = format!("{s}");
        assert!(output.contains("My Article"));
        assert!(output.contains("https://example.com"));
    }

    #[test]
    fn scraped_content_display_empty_title_shows_placeholder() {
        let s = make_scraped("", "https://example.com");
        let output = format!("{s}");
        assert!(output.contains("(untitled)"));
    }

    // -- ScrapedContent PartialEq tests --

    #[test]
    fn scraped_content_equal_when_identical() {
        let a = make_scraped("Title", "https://example.com");
        let b = make_scraped("Title", "https://example.com");
        assert_eq!(a, b);
    }

    #[test]
    fn scraped_content_not_equal_different_url() {
        let a = make_scraped("Title", "https://example.com/a");
        let b = make_scraped("Title", "https://example.com/b");
        assert_ne!(a, b);
    }

    #[test]
    fn scraped_content_not_equal_different_content() {
        let mut a = make_scraped("Title", "https://example.com");
        a.content = "one".to_string();
        let mut b = make_scraped("Title", "https://example.com");
        b.content = "two".to_string();
        assert_ne!(a, b);
    }

    #[test]
    fn test_document_chunk_from_scraped_content() {
        let scraped = ScrapedContent {
            title: "Test Title".to_string(),
            content: "Test content body".to_string(),
            url: ValidUrl::parse("https://example.com/article").unwrap(),
            excerpt: Some("An excerpt".to_string()),
            author: Some("Jane Doe".to_string()),
            date: Some("2024-06-01".to_string()),
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };

        let chunk = DocumentChunk::from_scraped_content(&scraped);

        assert_eq!(chunk.url, "https://example.com/article");
        assert_eq!(chunk.title, "Test Title");
        assert_eq!(chunk.content, "Test content body");
        assert!(chunk.metadata.contains_key("excerpt"));
        assert!(chunk.metadata.contains_key("author"));
        assert!(chunk.metadata.contains_key("date"));
        assert!(chunk.metadata.contains_key("domain"));
        assert_eq!(chunk.metadata["domain"], "example.com");
        assert!(chunk.embeddings.is_none());
    }

    #[test]
    fn test_document_chunk_validate_empty_content() {
        let chunk = DocumentChunk::new(uuid::Uuid::new_v4(), "https://example.com", "Title", "");
        assert!(matches!(
            chunk.validate(),
            Err(ValidationError::EmptyContent)
        ));
    }

    #[test]
    fn test_document_chunk_validate_empty_title() {
        let chunk = DocumentChunk::new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "",
            "content here",
        );
        assert!(matches!(chunk.validate(), Err(ValidationError::EmptyTitle)));
    }

    #[test]
    fn test_document_chunk_validate_invalid_url() {
        let chunk = DocumentChunk::new(uuid::Uuid::new_v4(), "not-a-url", "Title", "content here");
        assert!(matches!(
            chunk.validate(),
            Err(ValidationError::InvalidUrl(_))
        ));
    }

    // -- ScrapedContent Debug test --

    #[test]
    fn scraped_content_debug_contains_key_fields() {
        let s = make_scraped("Debug Test", "https://example.com");
        let dbg = format!("{s:?}");
        assert!(dbg.contains("Debug Test"));
        assert!(dbg.contains("example.com"));
        assert!(dbg.contains("ScrapedContent"));
    }

    // -- ExportFormat::parse_str mutation-killing tests --

    #[test]
    fn test_export_format_parse_str_all_variants() {
        assert_eq!(ExportFormat::parse_str("jsonl"), Ok(ExportFormat::Jsonl));
        assert_eq!(ExportFormat::parse_str("vector"), Ok(ExportFormat::Vector));
        assert_eq!(ExportFormat::parse_str("auto"), Ok(ExportFormat::Auto));
    }

    #[test]
    fn test_export_format_parse_str_case_insensitive() {
        assert_eq!(ExportFormat::parse_str("JSONL"), Ok(ExportFormat::Jsonl));
        assert_eq!(ExportFormat::parse_str("Vector"), Ok(ExportFormat::Vector));
        assert_eq!(ExportFormat::parse_str("AUTO"), Ok(ExportFormat::Auto));
    }

    #[test]
    fn test_export_format_parse_str_invalid_returns_error() {
        assert!(ExportFormat::parse_str("bogus").is_err());
        assert!(ExportFormat::parse_str("json").is_err());
        assert!(ExportFormat::parse_str("markdown").is_err());
        assert!(ExportFormat::parse_str("").is_err());
    }

    // -- DocumentChunk accessor mutation-killing tests --

    #[test]
    fn test_document_chunk_has_embeddings_true() {
        let chunk = DocumentChunk::test_new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "content",
        )
        .with_embeddings(vec![0.1, 0.2, 0.3]);
        assert!(chunk.has_embeddings());
    }

    #[test]
    fn test_document_chunk_has_embeddings_false() {
        let chunk = DocumentChunk::test_new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "content",
        );
        assert!(!chunk.has_embeddings());
    }

    #[test]
    fn test_document_chunk_text_length_nonempty() {
        let chunk = DocumentChunk::test_new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "hello world",
        );
        assert_eq!(chunk.text_length(), 11);
    }

    #[test]
    fn test_document_chunk_text_length_empty() {
        let chunk =
            DocumentChunk::test_new(uuid::Uuid::new_v4(), "https://example.com", "Title", "");
        assert_eq!(chunk.text_length(), 0);
    }

    // -- ExportState side-effect mutation-killing tests --

    #[test]
    fn test_export_state_mark_processed_increments_counter() {
        let mut state = ExportState::new("example.com");
        assert_eq!(state.total_exported, 0);

        state.mark_processed("https://example.com/page1");
        assert_eq!(state.total_exported, 1);
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_export_state_mark_processed_no_duplicate() {
        let mut state = ExportState::new("example.com");
        state.mark_processed("https://example.com/page1");
        state.mark_processed("https://example.com/page1");
        assert_eq!(state.total_exported, 1);
        assert_eq!(state.processed_urls.len(), 1);
    }

    #[test]
    fn test_export_state_mark_processed_multiple_urls() {
        let mut state = ExportState::new("example.com");
        state.mark_processed("https://example.com/page1");
        state.mark_processed("https://example.com/page2");
        state.mark_processed("https://example.com/page3");
        assert_eq!(state.total_exported, 3);
        assert!(state.is_processed("https://example.com/page1"));
        assert!(state.is_processed("https://example.com/page2"));
        assert!(!state.is_processed("https://example.com/other"));
    }

    #[test]
    fn test_export_state_update_timestamp() {
        let mut state = ExportState::new("example.com");
        assert!(state.last_export.is_none());

        state.update_timestamp();
        assert!(state.last_export.is_some());

        let ts1 = state.last_export.unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        state.update_timestamp();
        let ts2 = state.last_export.unwrap();
        assert!(ts2 >= ts1);
    }
}
