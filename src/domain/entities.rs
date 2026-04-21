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

/// Represents a downloaded asset (image or document)
///
/// Contains metadata about the original URL and local storage location.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<DownloadedAsset>,
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
pub enum ExportFormat {
    /// JSONL format (JSON Lines - one JSON object per line)
    /// Optimal for RAG pipelines and vector database ingestion
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
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
            correlation_id: None,
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
        Self::from_scraped_content_with_correlation(scraped, None)
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
                    "metadata key '{}' has empty value",
                    key
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
/// Stored at ~/.cache/rust-scraper/state/<domain>.json
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
}
