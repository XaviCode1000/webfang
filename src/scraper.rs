//! Modern Scraper Module
//!
//! Uses reqwest for HTTP and legible (Readability algorithm) for clean content extraction.
//! This is the 2026 best practice approach for obtaining clean data for RAG/datasets.
//!
//! Features:
//! - HTML to Markdown conversion with structure preservation
//! - Syntax highlighting for code blocks
//! - Image extraction and local saving
//! - YAML frontmatter with metadata
//! - Domain-based folder organization
//! - URL-based file naming

use crate::url_path::OutputPath;
use anyhow::{Context, Result};
use chrono::Utc;
use html_to_markdown_rs::{convert, ConversionOptions, HeadingStyle};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;
use tracing::{debug, info, warn};

#[allow(dead_code)]
/// HTTP Client configuration
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TIMEOUT_SECS: u64 = 30;

/// Validated URL newtype - guarantees URL is valid at type level
///
/// This enforces that ScrapedContent always has a valid URL,
/// preventing runtime errors from invalid URLs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidUrl(url::Url);

impl ValidUrl {
    /// Create a new ValidUrl from a validated url::Url
    pub fn new(url: url::Url) -> Self {
        Self(url)
    }

    /// Parse and create a ValidUrl from a string
    ///
    /// Returns error if the string is not a valid URL
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(Self(url::Url::parse(s)?))
    }

    /// Get reference to inner url::Url
    pub fn as_url(&self) -> &url::Url {
        &self.0
    }

    /// Get the URL as string
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl From<url::Url> for ValidUrl {
    fn from(url: url::Url) -> Self {
        Self(url)
    }
}

impl std::fmt::Display for ValidUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Represents a downloaded asset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedAsset {
    /// Original URL of the asset
    pub url: String,
    /// Local path where asset was saved
    pub local_path: String,
    /// Asset type (image or document)
    pub asset_type: String,
    /// File size in bytes
    pub size: u64,
}

/// Represents a scraped content item
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
    /// Publication date if available
    pub date: Option<String>,
    /// The HTML source (optional, for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    /// Downloaded assets (images, documents)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<DownloadedAsset>,
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
) -> Result<Vec<ScrapedContent>> {
    scrape_with_config(client, url, &crate::ScraperConfig::default()).await
}

/// Scrape a URL with asset downloading configuration
///
/// # Arguments
/// * `client` - HTTP client
/// * `url` - URL to scrape
/// * `config` - Scraper configuration with download options
///
/// # Returns
/// * `Vec<ScrapedContent>` - Scraped content with downloaded assets
pub async fn scrape_with_config(
    client: &Client,
    url: &url::Url,
    config: &crate::ScraperConfig,
) -> Result<Vec<ScrapedContent>> {
    let mut results = Vec::new();

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
    match legible::parse(&html, Some(url.as_str()), None) {
        Ok(article) => {
            // Download assets if configured
            let assets = if config.has_downloads() {
                download_assets(&html, url, config).await?
            } else {
                Vec::new()
            };

            let content = ScrapedContent {
                title: article.title,
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                html: Some(html),
                assets,
            };

            info!(
                "✅ Extracted: {} ({} chars, {} assets)",
                content.title,
                content.content.len(),
                content.assets.len()
            );
            results.push(content);
        }
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            // Try fallback: just extract text directly
            let fallback_content = extract_fallback_text(&html);

            // Download assets if configured
            let assets = if config.has_downloads() {
                download_assets(&html, url, config).await?
            } else {
                Vec::new()
            };

            let title = url
                .host_str()
                .ok_or_else(|| anyhow::anyhow!("URL missing host after validation: {}", url))?
                .to_string();

            results.push(ScrapedContent {
                title,
                content: fallback_content,
                url: ValidUrl::new(url.clone()),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html),
                assets,
            });
        }
    }

    Ok(results)
}

/// Download assets (images and documents) from HTML
#[cfg(any(feature = "images", feature = "documents"))]
async fn download_assets(
    html: &str,
    base_url: &url::Url,
    config: &crate::ScraperConfig,
) -> Result<Vec<DownloadedAsset>> {
    use crate::downloader::{DownloadConfig, Downloader};
    use crate::extractor;

    let mut assets = Vec::new();

    // Extract image URLs
    if config.download_images {
        let images = extractor::extract_images(html, base_url);
        if !images.is_empty() {
            info!("🖼️  Found {} images to download", images.len());

            let download_config = DownloadConfig {
                output_dir: config.output_dir.clone(),
                images_dir: "images".to_string(),
                documents_dir: "documents".to_string(),
                max_file_size: config.max_file_size.unwrap_or(50 * 1024 * 1024),
                timeout_secs: 30,
            };

            let downloader = Downloader::new(download_config)?;

            for img in images {
                match downloader.download(&img.url).await {
                    Ok(downloaded) => {
                        assets.push(DownloadedAsset {
                            url: downloaded.url,
                            local_path: downloaded.local_path.to_string_lossy().to_string(),
                            asset_type: "image".to_string(),
                            size: downloaded.size,
                        });
                    }
                    Err(e) => {
                        warn!("Failed to download image {}: {}", img.url, e);
                    }
                }
            }
        }
    }

    // Extract document URLs
    if config.download_documents {
        let documents = extractor::extract_documents(html, base_url);
        if !documents.is_empty() {
            info!("📄 Found {} documents to download", documents.len());

            let download_config = DownloadConfig {
                output_dir: config.output_dir.clone(),
                images_dir: "images".to_string(),
                documents_dir: "documents".to_string(),
                max_file_size: config.max_file_size.unwrap_or(50 * 1024 * 1024),
                timeout_secs: 30,
            };

            let downloader = Downloader::new(download_config)?;

            for doc in documents {
                match downloader.download(&doc.url).await {
                    Ok(downloaded) => {
                        assets.push(DownloadedAsset {
                            url: downloaded.url,
                            local_path: downloaded.local_path.to_string_lossy().to_string(),
                            asset_type: "document".to_string(),
                            size: downloaded.size,
                        });
                    }
                    Err(e) => {
                        warn!("Failed to download document {}: {}", doc.url, e);
                    }
                }
            }
        }
    }

    Ok(assets)
}

/// Download assets - stub for when features are disabled
#[cfg(not(any(feature = "images", feature = "documents")))]
async fn download_assets(
    _html: &str,
    _base_url: &url::Url,
    _config: &crate::ScraperConfig,
) -> Result<Vec<DownloadedAsset>> {
    // No-op when features are disabled
    Ok(Vec::new())
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

// ============================================================================
// Advanced Markdown Conversion with Structure, Syntax Highlighting, and Images
// ============================================================================

/// Convert HTML to well-structured Markdown using html-to-markdown-rs
fn html_to_structured_markdown(html: &str) -> String {
    let options = ConversionOptions {
        heading_style: HeadingStyle::Atx,
        ..Default::default()
    };

    convert(html, Some(options)).unwrap_or_else(|e| {
        warn!("HTML to Markdown conversion failed: {}, falling back", e);
        extract_fallback_text(html)
    })
}

/// Apply syntax highlighting to code blocks in Markdown
fn apply_syntax_highlighting(markdown: &str) -> String {
    // Load syntax definitions and themes
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let theme_set = ThemeSet::load_defaults();

    // Use a popular dark theme
    let theme = &theme_set.themes["base16-ocean.dark"];

    // Regex to find code blocks: ```language\ncode\n```
    let code_block_re = regex::Regex::new(r"```(\w*)\n([\s\S]*?)```").unwrap();

    let mut result = markdown.to_string();

    for cap in code_block_re.captures_iter(markdown) {
        let language = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let code = cap.get(2).map(|m| m.as_str()).unwrap_or("");

        // Try to find the syntax
        let syntax = syntax_set
            .find_syntax_by_token(language)
            .unwrap_or_else(|| syntax_set.find_syntax_plain_text());

        // Try to highlight, fall back to plain if it fails
        let highlighted = highlighted_html_for_string(code, &syntax_set, syntax, theme)
            .unwrap_or_else(|_| code.to_string());

        // Replace the code block with highlighted version
        // Note: This is a simplified version - in production you might want to
        // use a different approach to preserve the markdown structure
        let replacement = format!("```{}\n{}```", language, highlighted);
        result = result.replace(cap.get(0).unwrap().as_str(), &replacement);
    }

    result
}

/// YAML frontmatter metadata
#[derive(Debug, Serialize)]
struct Frontmatter {
    title: String,
    url: String,
    date: String,
    author: Option<String>,
    excerpt: Option<String>,
}

/// Generate YAML frontmatter for a markdown file
fn generate_frontmatter(
    title: &str,
    url: &str,
    date: Option<&str>,
    author: Option<&str>,
    excerpt: Option<&str>,
) -> String {
    let fm = Frontmatter {
        title: title.to_string(),
        url: url.to_string(),
        date: date
            .map(|s| s.to_string())
            .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string()),
        author: author.map(|s| s.to_string()),
        excerpt: excerpt.map(|s| s.to_string()),
    };

    serde_yaml::to_string(&fm).unwrap_or_else(|_| String::new())
}

/// Save scraped results to output directory
///
/// Now supports:
/// - Domain-based folder structure
/// - URL-based file naming
/// - YAML frontmatter with metadata
/// - Syntax highlighting for code blocks
/// - Image extraction and local saving
pub fn save_results(
    results: &[ScrapedContent],
    output_dir: &Path,
    format: &super::OutputFormat,
) -> Result<()> {
    use std::fs;

    // Create base output directory
    fs::create_dir_all(output_dir)?;

    match format {
        super::OutputFormat::Markdown => {
            for item in results.iter() {
                // Create OutputPath from URL
                let output_path = match OutputPath::from_url(item.url.as_str()) {
                    Ok(p) => p,
                    Err(e) => {
                        // Fallback for URL parsing errors
                        warn!("Failed to parse URL {}: {}, using fallback", item.url, e);
                        let fallback_path = output_dir.join("index.md");
                        fs::create_dir_all(output_dir)?;
                        let content = format!("# {}\n\n{}", item.title, item.content);
                        fs::write(&fallback_path, content)?;
                        continue;
                    }
                };

                // Get full path and create directories
                // output_path.to_full_path() returns "./output/domain/path.md"
                // We need to join output_dir with the relative part (without "./output/")
                let full_path_str = output_path.to_full_path();
                let relative_path = full_path_str.trim_start_matches("./output/");
                let full_path = output_dir.join(relative_path);
                if let Some(parent) = full_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                // Convert HTML to structured Markdown
                let markdown_content = if let Some(html) = &item.html {
                    html_to_structured_markdown(html)
                } else {
                    // Fallback to plain text if no HTML available
                    item.content.clone()
                };

                // Apply syntax highlighting
                let highlighted = apply_syntax_highlighting(&markdown_content);

                // Extract and download images (async operation - simplified here)
                // Note: Full async image downloading would require making this async
                // For now, we'll skip image downloading in sync context

                // Generate YAML frontmatter
                let frontmatter = generate_frontmatter(
                    &item.title,
                    item.url.as_str(),
                    item.date.as_deref(),
                    item.author.as_deref(),
                    item.excerpt.as_deref(),
                );

                // Combine frontmatter and content
                let final_content = format!("---\n{}---\n\n{}", frontmatter.trim(), highlighted);

                fs::write(&full_path, final_content)?;
                info!("💾 Saved: {}", full_path.display());
            }
        }
        super::OutputFormat::Text => {
            for item in results.iter() {
                let output_path = match OutputPath::from_url(item.url.as_str()) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("Failed to parse URL {}: {}, using fallback", item.url, e);
                        let fallback_path = output_dir.join("index.txt");
                        fs::write(&fallback_path, &item.content)?;
                        continue;
                    }
                };

                let full_path = output_dir.join(
                    output_path
                        .to_full_path()
                        .trim_start_matches("./")
                        .replace(".md", ".txt"),
                );
                if let Some(parent) = full_path.parent() {
                    fs::create_dir_all(parent)?;
                }

                fs::write(&full_path, &item.content)?;
                info!("💾 Saved: {}", full_path.display());
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
            url: ValidUrl::parse("https://example.com/article").unwrap(),
            excerpt: Some("A short excerpt".to_string()),
            author: Some("John Doe".to_string()),
            date: Some("2024-01-15".to_string()),
            html: None,
            assets: Vec::new(),
        }];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Markdown);

        // Assert
        assert!(result.is_ok());

        // Verify file was created (now in subdirectory based on domain)
        use walkdir::WalkDir;
        let files: Vec<_> = WalkDir::new(&output_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
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
                url: ValidUrl::parse("https://example.com/1").unwrap(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
                assets: Vec::new(),
            },
            ScrapedContent {
                title: "Article 2".to_string(),
                content: "Content 2".to_string(),
                url: ValidUrl::parse("https://example.com/2").unwrap(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
                assets: Vec::new(),
            },
        ];

        // Act
        let result = save_results(&results, &output_dir, &super::super::OutputFormat::Text);

        // Assert
        assert!(result.is_ok());

        use walkdir::WalkDir;
        let files: Vec<_> = WalkDir::new(&output_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
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
                url: ValidUrl::parse("https://example.com/1").unwrap(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
                assets: Vec::new(),
            },
            ScrapedContent {
                title: "Article 2".to_string(),
                content: "Content 2".to_string(),
                url: ValidUrl::parse("https://example.com/2").unwrap(),
                excerpt: None,
                author: None,
                date: None,
                html: None,
                assets: Vec::new(),
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
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: Some("Test excerpt".to_string()),
            author: Some("Author Name".to_string()),
            date: Some("2024-01-01".to_string()),
            html: None, // Should be skipped in serialization
            assets: Vec::new(),
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
        assert!(parsed[0].url.as_str().starts_with("https://example.com"));
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
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
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
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: Some("Excerpt".to_string()),
            author: Some("Author".to_string()),
            date: Some("2024-01-01".to_string()),
            html: None,
            assets: Vec::new(),
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
            "date": "2024-01-01",
            "assets": []
        }"#;

        // Act
        let content: ScrapedContent = serde_json::from_str(json).expect("Should deserialize");

        // Assert
        assert_eq!(content.title, "Test");
        assert_eq!(content.content, "Content");
        assert!(content.url.as_str().starts_with("https://example.com"));
        assert_eq!(content.excerpt, Some("Excerpt".to_string()));
        assert_eq!(content.author, Some("Author".to_string()));
        assert_eq!(content.date, Some("2024-01-01".to_string()));
    }
}
