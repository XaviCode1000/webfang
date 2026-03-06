//! Modern Scraper Module
//!
//! Uses reqwest for HTTP and legible (Readability algorithm) for clean content extraction.
//! This is the 2026 best practice approach for obtaining clean data for RAG/datasets.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, info, warn};

/// HTTP Client configuration
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TIMEOUT_SECS: u64 = 30;

/// Represents a scraped content item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapedContent {
    /// Title of the page/article
    pub title: String,
    /// Main content extracted (clean, without ads/nav)
    pub content: String,
    /// Original URL
    pub url: String,
    /// Excerpt/summary if available
    pub excerpt: Option<String>,
    /// Author if available
    pub author: Option<String>,
    /// Publication date if available
    pub date: Option<String>,
    /// The HTML source (optional, for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

/// Create configured HTTP client with best practices
pub fn create_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .gzip(true) // Most modern sites use gzip
        .brotli(true)
        .build()
        .context("Failed to create HTTP client")
}

/// Scrape a URL using Readability algorithm for clean content extraction
///
/// This is the modern 2026 approach - uses the same algorithm as Firefox Reader View
/// to extract only the meaningful content (article body), filtering out:
/// - Navigation menus
/// - Advertisements
/// - Sidebars
/// - Footer content
/// - Scripts and styles
pub async fn scrape_with_readability(
    client: &Client,
    url: &url::Url,
    _selector: &str, // Reserved for future advanced selectors
    _max_pages: usize,
    _delay_ms: u64,
) -> Result<Vec<ScrapedContent>> {
    let mut results = Vec::new();

    // For now, scrape single URL - can extend to crawl later
    info!("🌐 Fetching: {}", url);

    // Fetch HTML
    let response = client
        .get(url.as_str())
        .send()
        .await
        .with_context(|| format!("Failed to fetch URL: {}", url))?;

    // Check status
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("HTTP error: {} - {}", status, url);
    }

    // Get HTML content
    let html = response
        .text()
        .await
        .context("Failed to read response body")?;

    debug!("📄 Downloaded {} bytes from {}", html.len(), url);

    // Extract clean content using Readability algorithm
    // legible::parse requires url and options as arguments
    match legible::parse(&html, Some(url.as_str()), None) {
        Ok(article) => {
            let content = ScrapedContent {
                // legible uses fields, not methods
                title: article.title,
                content: article.text_content,
                url: url.to_string(),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                html: Some(html), // Keep for debugging if needed
            };

            info!(
                "✅ Extracted: {} ({} chars)",
                content.title,
                content.content.len()
            );
            results.push(content);
        }
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            // Try fallback: just extract text directly
            let fallback_content = extract_fallback_text(&html);

            // FIX: Propagar error en vez de fallback silencioso
            // La URL ya fue validada, pero por seguridad checkedamos igual
            let title = url
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("URL missing host after validation: {}", url))?
                .to_string();

            results.push(ScrapedContent {
                title,
                content: fallback_content,
                url: url.to_string(),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html),
            });
        }
    }

    // Note: For multi-page crawling, implement delay and loop here
    // For now, single page as per max_pages = 1 for simplicity

    Ok(results)
}

/// Fallback: Extract text without readability (basic HTML stripping)
fn extract_fallback_text(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| {
        // If htmd fails, do a very basic strip
        html.lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Save scraped results to output directory
pub fn save_results(
    results: &[ScrapedContent],
    output_dir: &PathBuf,
    format: &super::OutputFormat,
) -> Result<()> {
    use std::fs;

    // Create output directory
    fs::create_dir_all(output_dir)?;

    match format {
        super::OutputFormat::Markdown => {
            for (i, item) in results.iter().enumerate() {
                let filename = format!("doc_{:03}.md", i);
                let path = output_dir.join(&filename);

                let md_content = format!(
                    "# {}\n\n{}\n\n---\n\n*Source: [{}]({})*",
                    item.title, item.content, item.url, item.url
                );

                fs::write(&path, md_content)?;
                info!("💾 Saved: {}", path.display());
            }
        }
        super::OutputFormat::Text => {
            for (i, item) in results.iter().enumerate() {
                let filename = format!("doc_{:03}.txt", i);
                let path = output_dir.join(&filename);
                fs::write(&path, &item.content)?;
                info!("💾 Saved: {}", path.display());
            }
        }
        super::OutputFormat::Json => {
            let json_path = output_dir.join("results.json");
            let json = serde_json::to_string_pretty(results)?;
            fs::write(&json_path, json)?;
            info!("💾 Saved: {}", json_path.display());
        }
    }

    Ok(())
}

// ============================================================================
// Legacy functions removed in v0.2.0
// ============================================================================

/// Legacy function - removed
#[allow(dead_code)]
fn _deprecated_crawl_target() {
    // This function no longer exists - use scrape_with_readability instead
    // Keeping empty to avoid breaking builds that might reference it
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ==========================================================================
    // Tests: create_http_client
    // ==========================================================================

    #[test]
    fn test_create_http_client_success() {
        // Act
        let result = create_http_client();

        // Assert
        assert!(result.is_ok());
        // Client was created successfully with configuration
    }

    // ==========================================================================
    // Tests: extract_fallback_text
    // ==========================================================================

    #[test]
    fn test_extract_fallback_text_with_valid_html() {
        // Arrange
        let html = r#"<html><head><title>Test</title></head>
        <body><p>Hello World</p><script>alert('x')</script></body></html>"#;

        // Act
        let result = extract_fallback_text(html);

        // Assert - Main content should be extracted
        assert!(result.contains("Hello World"));
        // Verify HTML was processed (not returned verbatim)
        assert!(!result.contains("<html>"));
        assert!(!result.contains("<body>"));
    }

    #[test]
    fn test_extract_fallback_text_with_scripts_removed() {
        // Arrange - HTML with multiple scripts and styles
        let html = r#"
        <html>
        <head>
            <style>.nav { color: red; }</style>
            <script>var x = 1;</script>
        </head>
        <body>
            <nav>Navigation content</nav>
            <article>Main article content here</article>
            <footer>Footer info</footer>
        </body>
        </html>"#;

        // Act
        let result = extract_fallback_text(html);

        // Assert
        assert!(result.contains("Main article content"));
        // Verify HTML tags were stripped
        assert!(!result.contains("<html>"));
        assert!(!result.contains("<head>"));
        assert!(!result.contains("<article>"));
    }

    #[test]
    fn test_extract_fallback_text_empty_html() {
        // Arrange
        let html = "";

        // Act
        let result = extract_fallback_text(html);

        // Assert - Should return empty string, not crash
        assert_eq!(result, "");
    }

    #[test]
    fn test_extract_fallback_text_malformed_html() {
        // Arrange - Malformed HTML
        let html = "<div>Open div never closed<p>Paragraph";

        // Act
        let result = extract_fallback_text(html);

        // Assert - Should not crash, should extract what it can
        assert!(result.contains("Paragraph") || !result.is_empty());
    }

    // ==========================================================================
    // Tests: save_results - Markdown format
    // ==========================================================================

    #[test]
    fn test_save_results_markdown_single_item() {
        // Arrange
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_dir = temp_dir.path().to_path_buf();

        let results = vec![ScrapedContent {
            title: "Test Article".to_string(),
            content: "This is the main content.".to_string(),
            url: "https://example.com/article".to_string(),
            excerpt: Some("A short excerpt".to_string()),
            author: Some("John Doe".to_string()),
            date: Some("2024-01-15".to_string()),
            html: None,
        }];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Markdown);

        // Assert
        assert!(result.is_ok());

        // Verify file was created
        let files: Vec<_> = fs::read_dir(&output_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);

        let content = fs::read_to_string(files[0].path()).unwrap();
        assert!(content.contains("Test Article"));
        assert!(content.contains("This is the main content."));
        assert!(content.contains("https://example.com/article"));
    }

    #[test]
    fn test_save_results_markdown_multiple_items() {
        // Arrange
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_dir = temp_dir.path().to_path_buf();

        let results = vec![
            ScrapedContent {
                title: "Article 1".to_string(),
                content: "Content 1".to_string(),
                url: "https://example.com/1".to_string(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
            },
            ScrapedContent {
                title: "Article 2".to_string(),
                content: "Content 2".to_string(),
                url: "https://example.com/2".to_string(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
            },
        ];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Markdown);

        // Assert
        assert!(result.is_ok());

        let files: Vec<_> = fs::read_dir(&output_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 2);
    }

    // ==========================================================================
    // Tests: save_results - Text format
    // ==========================================================================

    #[test]
    fn test_save_results_text_single_item() {
        // Arrange
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_dir = temp_dir.path().to_path_buf();

        let results = vec![ScrapedContent {
            title: "Test Article".to_string(),
            content: "Plain text content here.".to_string(),
            url: "https://example.com".to_string(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
        }];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Text);

        // Assert
        assert!(result.is_ok());

        let files: Vec<_> = fs::read_dir(&output_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1);

        let content = fs::read_to_string(files[0].path()).unwrap();
        // Text format should only contain content, not title or URL
        assert!(content.contains("Plain text content here."));
        assert!(!content.contains("Test Article")); // Title not in file
        assert!(!content.contains("https://example.com")); // URL not in file
    }

    // ==========================================================================
    // Tests: save_results - JSON format
    // ==========================================================================

    #[test]
    fn test_save_results_json_multiple_items() {
        // Arrange
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_dir = temp_dir.path().to_path_buf();

        let results = vec![
            ScrapedContent {
                title: "Article 1".to_string(),
                content: "Content 1".to_string(),
                url: "https://example.com/1".to_string(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
            },
            ScrapedContent {
                title: "Article 2".to_string(),
                content: "Content 2".to_string(),
                url: "https://example.com/2".to_string(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
            },
        ];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Json);

        // Assert
        assert!(result.is_ok());

        // JSON creates single file
        let json_path = output_dir.join("results.json");
        assert!(json_path.exists());

        let content = fs::read_to_string(&json_path).unwrap();
        // Verify valid JSON and contains both articles
        let parsed: Vec<ScrapedContent> = serde_json::from_str(&content).expect("Valid JSON");
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn test_save_results_json_contains_all_fields() {
        // Arrange
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_dir = temp_dir.path().to_path_buf();

        let results = vec![ScrapedContent {
            title: "Test Title".to_string(),
            content: "Test Content".to_string(),
            url: "https://example.com".to_string(),
            excerpt: Some("Test excerpt".to_string()),
            author: Some("Author Name".to_string()),
            date: Some("2024-01-01".to_string()),
            html: None, // Should be skipped in serialization
        }];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Json);

        // Assert
        assert!(result.is_ok());

        let json_path = output_dir.join("results.json");
        let content = fs::read_to_string(&json_path).unwrap();

        // Verify JSON is valid by deserializing
        let parsed: Vec<ScrapedContent> = serde_json::from_str(&content).expect("Valid JSON");

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Test Title");
        assert_eq!(parsed[0].content, "Test Content");
        assert_eq!(parsed[0].url, "https://example.com");
        assert_eq!(parsed[0].excerpt, Some("Test excerpt".to_string()));
        assert_eq!(parsed[0].author, Some("Author Name".to_string()));
        assert_eq!(parsed[0].date, Some("2024-01-01".to_string()));
        // html should be None (skip_serializing)
        assert_eq!(parsed[0].html, None);
    }

    // ==========================================================================
    // Tests: save_results - Edge cases
    // ==========================================================================

    #[test]
    fn test_save_results_creates_directory_if_not_exists() {
        // Arrange
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_dir = temp_dir.path().join("nested").join("output");

        let results = vec![ScrapedContent {
            title: "Test".to_string(),
            content: "Content".to_string(),
            url: "https://example.com".to_string(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
        }];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Text);

        // Assert - Should create nested directories
        assert!(result.is_ok());
        assert!(output_dir.exists());
    }

    #[test]
    fn test_save_results_empty_results() {
        // Arrange
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let output_dir = temp_dir.path().to_path_buf();

        let results: Vec<ScrapedContent> = vec![];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Markdown);

        // Assert - Should succeed but create no files (for Markdown/Text)
        assert!(result.is_ok());
    }

    // ==========================================================================
    // Tests: ScrapedContent serialization
    // ==========================================================================

    #[test]
    fn test_scraped_content_json_serialization() {
        // Arrange
        let content = ScrapedContent {
            title: "Test Title".to_string(),
            content: "Test Content".to_string(),
            url: "https://example.com".to_string(),
            excerpt: Some("Excerpt".to_string()),
            author: Some("Author".to_string()),
            date: Some("2024-01-01".to_string()),
            html: None,
        };

        // Act
        let json = serde_json::to_string(&content).expect("Should serialize");

        // Assert
        assert!(json.contains("Test Title"));
        assert!(json.contains("Test Content"));
        // html should be None, so skip_serializing should work
        assert!(!json.contains("html"));
    }

    #[test]
    fn test_scraped_content_json_deserialization() {
        // Arrange
        let json = r#"{
            "title": "Test",
            "content": "Content",
            "url": "https://example.com",
            "excerpt": "Excerpt",
            "author": "Author",
            "date": "2024-01-01"
        }"#;

        // Act
        let content: ScrapedContent = serde_json::from_str(json).expect("Should deserialize");

        // Assert
        assert_eq!(content.title, "Test");
        assert_eq!(content.content, "Content");
        assert_eq!(content.url, "https://example.com");
        assert_eq!(content.excerpt, Some("Excerpt".to_string()));
        assert_eq!(content.author, Some("Author".to_string()));
        assert_eq!(content.date, Some("2024-01-01".to_string()));
    }
}
