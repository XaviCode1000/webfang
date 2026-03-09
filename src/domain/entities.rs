//! Domain entities — Core business types
//!
//! These are the fundamental data structures used throughout the application.
//! They are serializable for persistence but contain no business logic.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::ValidUrl;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, clap::ValueEnum)]
pub enum ExportFormat {
    /// Markdown format (for human-readable output)
    Markdown,
    /// JSONL format (JSON Lines - one JSON object per line)
    Jsonl,
    /// Plain text format
    Text,
    /// Structured JSON format
    Json,
    /// Auto-detect format from existing files
    Auto,
    /// Zvec format (for vector database imports)
    /// Schema: id (UUID), text (String), embedding (Vec<f32>)
    Zvec,
}

impl ExportFormat {
    /// Returns the file extension for this format
    #[must_use]
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Jsonl => "jsonl",
            Self::Text => "txt",
            Self::Json => "json",
            Self::Auto => "auto",
            Self::Zvec => "zvec",
        }
    }

    /// Returns a human-readable name for this format
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Jsonl => "JSONL",
            Self::Text => "Text",
            Self::Json => "Json",
            Self::Auto => "Auto",
            Self::Zvec => "Zvec",
        }
    }
}

/// A document chunk ready for export to RAG pipelines
///
/// Represents a single unit of content that can be:
/// - Embedded in a vector database
/// - Exported in various formats (JSONL, Zvec, Markdown)
///
/// The embeddings field is optional because in the initial scraping phase
/// the AI model may not be available yet. It can be populated later
/// by a separate embedding pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
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
}

impl DocumentChunk {
    /// Create a new DocumentChunk from ScrapedContent
    ///
    /// This is the main conversion method from the scraper's output
    /// to the RAG pipeline's input format.
    #[must_use]
    pub fn from_scraped_content(scraped: &ScrapedContent) -> Self {
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

    /// Get the text length (character count)
    #[must_use]
    pub fn text_length(&self) -> usize {
        self.content.len()
    }

    /// Check if this chunk has embeddings
    #[must_use]
    pub fn has_embeddings(&self) -> bool {
        self.embeddings.is_some()
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
}
