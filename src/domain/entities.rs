//! Domain entities — Core business types
//!
//! These are the fundamental data structures used throughout the application.
//! They are serializable for persistence but contain no business logic.

use crate::domain::ValidUrl;
use serde::{Deserialize, Serialize};

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
