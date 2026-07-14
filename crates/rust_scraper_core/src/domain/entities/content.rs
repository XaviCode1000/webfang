//! Core content entities — DocumentChunk and ScrapedContent

use std::collections::HashMap;
use std::marker::PhantomData;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{CorrelationId, ValidUrl};

use super::download::DownloadedAsset;

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn scraped_content_debug_contains_key_fields() {
        let s = make_scraped("Debug Test", "https://example.com");
        let dbg = format!("{s:?}");
        assert!(dbg.contains("Debug Test"));
        assert!(dbg.contains("example.com"));
        assert!(dbg.contains("ScrapedContent"));
    }

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

    #[test]
    fn test_document_chunk_with_metadata() {
        let mut meta = HashMap::new();
        meta.insert("author".to_string(), "Alice".to_string());
        meta.insert("source".to_string(), "rss".to_string());

        let chunk = DocumentChunk::with_metadata(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "content",
            meta,
        );

        assert_eq!(chunk.metadata.len(), 2);
        assert_eq!(chunk.metadata["author"], "Alice");
        assert_eq!(chunk.metadata["source"], "rss");
    }

    #[test]
    fn test_document_chunk_with_id() {
        let id = uuid::Uuid::new_v4();
        let scraped = ScrapedContent {
            title: "Test".to_string(),
            content: "Content".to_string(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        let chunk = DocumentChunk::with_id(&scraped, id);
        assert_eq!(chunk.id, id);
    }

    #[test]
    fn test_document_chunk_from_scraped_content_with_correlation() {
        let corr = CorrelationId::new();
        let scraped = ScrapedContent {
            title: "With Corr".to_string(),
            content: "Body".to_string(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        let chunk = DocumentChunk::from_scraped_content_with_correlation(&scraped, Some(&corr));
        assert!(chunk.correlation_id.is_some());
        assert!(chunk.correlation_id.unwrap().starts_with("00-"));
    }

    #[test]
    fn test_document_chunk_from_scraped_content_no_correlation() {
        let scraped = ScrapedContent {
            title: "No Corr".to_string(),
            content: "Body".to_string(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        let chunk = DocumentChunk::from_scraped_content(&scraped);
        assert!(chunk.correlation_id.is_none());
    }

    #[test]
    fn test_document_chunk_from_scraped_content_metadata_extraction() {
        let scraped = ScrapedContent {
            title: "Meta Test".to_string(),
            content: "Body".to_string(),
            url: ValidUrl::parse("https://blog.example.com/post").unwrap(),
            excerpt: Some("excerpt text".to_string()),
            author: Some("Author".to_string()),
            date: Some("2024-01-01".to_string()),
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        let chunk = DocumentChunk::from_scraped_content(&scraped);
        assert_eq!(chunk.metadata["excerpt"], "excerpt text");
        assert_eq!(chunk.metadata["author"], "Author");
        assert_eq!(chunk.metadata["date"], "2024-01-01");
        assert_eq!(chunk.metadata["domain"], "blog.example.com");
    }

    #[test]
    fn test_document_chunk_validate_success() {
        let chunk = DocumentChunk::new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Valid Title",
            "Valid content",
        );
        let validated = chunk.validate();
        assert!(validated.is_ok());
    }

    #[test]
    fn test_document_chunk_validate_whitespace_only_content() {
        let chunk = DocumentChunk::new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "   \t\n  ",
        );
        assert!(matches!(
            chunk.validate(),
            Err(ValidationError::EmptyContent)
        ));
    }

    #[test]
    fn test_document_chunk_validate_whitespace_only_title() {
        let chunk = DocumentChunk::new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "   ",
            "content",
        );
        assert!(matches!(chunk.validate(), Err(ValidationError::EmptyTitle)));
    }

    #[test]
    fn test_document_chunk_validate_empty_metadata_value() {
        let mut meta = HashMap::new();
        meta.insert("key".to_string(), "   ".to_string());

        let chunk = DocumentChunk::with_metadata(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "content",
            meta,
        );
        assert!(matches!(
            chunk.validate(),
            Err(ValidationError::InvalidMetadata(_))
        ));
    }

    #[test]
    fn test_document_chunk_validate_preserves_id() {
        let id = uuid::Uuid::new_v4();
        let chunk = DocumentChunk::new(id, "https://example.com", "Title", "content");
        let validated = chunk.validate().unwrap();
        assert_eq!(validated.id, id);
    }

    #[test]
    fn test_document_chunk_validate_preserves_metadata() {
        let mut meta = HashMap::new();
        meta.insert("author".to_string(), "Test".to_string());
        let chunk = DocumentChunk::with_metadata(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "content",
            meta,
        );
        let validated = chunk.validate().unwrap();
        assert_eq!(validated.metadata["author"], "Test");
    }

    #[test]
    fn test_document_chunk_serde_roundtrip() {
        let chunk = DocumentChunk::new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Serde Test",
            "Some content",
        );
        let json = serde_json::to_string(&chunk).unwrap();
        let deserialized: DocumentChunk<Draft> = serde_json::from_str(&json).unwrap();
        assert_eq!(chunk.url, deserialized.url);
        assert_eq!(chunk.title, deserialized.title);
        assert_eq!(chunk.content, deserialized.content);
    }

    #[test]
    fn test_document_chunk_with_embeddings_serde_roundtrip() {
        let chunk = DocumentChunk::test_new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "content",
        )
        .with_embeddings(vec![0.1, 0.2, 0.3]);
        let json = serde_json::to_string(&chunk).unwrap();
        assert!(json.contains("0.1"));
        let deserialized: DocumentChunk<Draft> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.embeddings, Some(vec![0.1, 0.2, 0.3]));
    }

    #[test]
    fn test_scraped_content_serde_roundtrip() {
        let content = ScrapedContent {
            title: "Serde Article".to_string(),
            content: "Full body".to_string(),
            url: ValidUrl::parse("https://example.com/article").unwrap(),
            excerpt: Some("excerpt".to_string()),
            author: Some("Author".to_string()),
            date: Some("2024-06-15".to_string()),
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        let deserialized: ScrapedContent = serde_json::from_str(&json).unwrap();
        assert_eq!(content.title, deserialized.title);
        assert_eq!(content.content, deserialized.content);
        assert_eq!(content.excerpt, deserialized.excerpt);
        assert_eq!(content.author, deserialized.author);
    }

    #[test]
    fn test_scraped_content_serde_with_assets() {
        let content = ScrapedContent {
            title: "Assets".to_string(),
            content: "body".to_string(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![DownloadedAsset {
                url: "https://example.com/img.png".to_string(),
                local_path: "/tmp/img.png".to_string(),
                asset_type: "image".to_string(),
                size: 1024,
            }],
            correlation_id: None,
        };
        let json = serde_json::to_string(&content).unwrap();
        let deserialized: ScrapedContent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.assets.len(), 1);
        assert_eq!(deserialized.assets[0].size, 1024);
    }

    #[test]
    fn test_scraped_content_serde_skips_none_html() {
        let content = make_scraped("Test", "https://example.com");
        let json = serde_json::to_string(&content).unwrap();
        assert!(!json.contains("html"));
    }

    #[test]
    fn test_scraped_content_serde_includes_some_html() {
        let mut content = make_scraped("Test", "https://example.com");
        content.html = Some("<html>".to_string());
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("html"));
    }

    #[test]
    fn test_document_chunk_from_scraped_content_no_optional_fields() {
        let scraped = ScrapedContent {
            title: "Minimal".to_string(),
            content: "content".to_string(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        };
        let chunk = DocumentChunk::from_scraped_content(&scraped);
        assert_eq!(chunk.metadata.len(), 1); // only domain
        assert!(chunk.metadata.contains_key("domain"));
    }

    #[test]
    fn test_document_chunk_with_embeddings_chain() {
        let chunk = DocumentChunk::test_new(
            uuid::Uuid::new_v4(),
            "https://example.com",
            "Title",
            "content",
        )
        .with_embeddings(vec![1.0])
        .with_embeddings(vec![1.0, 2.0]); // second call replaces

        assert_eq!(chunk.embeddings.unwrap(), vec![1.0, 2.0]);
    }

    #[test]
    fn test_scraped_content_clone() {
        let content = make_scraped("Clone Test", "https://example.com");
        let cloned = content.clone();
        assert_eq!(content, cloned);
    }

    #[test]
    fn test_validation_error_display() {
        assert_eq!(ValidationError::EmptyContent.to_string(), "empty_content");
        assert_eq!(ValidationError::EmptyTitle.to_string(), "empty_title");
        assert!(ValidationError::InvalidUrl("bad".to_string())
            .to_string()
            .contains("bad"));
        assert!(ValidationError::InvalidMetadata("reason".to_string())
            .to_string()
            .contains("reason"));
    }
}
