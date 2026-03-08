//! Rust Scraper Library
//!
//! Modern web scraper for RAG datasets with clean content extraction.
//!
//! # Architecture
//!
//! Following Clean Architecture:
//! - **Domain**: Core entities (ScrapedContent, ValidUrl) — pure business logic
//! - **Application**: Use cases (scraping, HTTP client) — orchestration
//! - **Infrastructure**: Implementations (HTTP, FS, converters) — technical details
//! - **Adapters**: External integrations (downloaders, extractors) — feature-gated
//!
//! # Examples
//!
//! ```no_run
//! use rust_scraper::{create_http_client, scrape_with_readability, ScraperConfig};
//!
//! # #[tokio::main]
//! # async fn main() -> anyhow::Result<()> {
//! let client = create_http_client()?;
//! let url = url::Url::parse("https://example.com")?;
//! let config = ScraperConfig::default();
//! let results = scrape_with_readability(&client, &url).await?;
//! # Ok(())
//! # }
//! ```

pub mod config;
pub mod error;

// Domain layer — Core business entities
pub mod domain;
pub use domain::{
    ContentType, CrawlError, CrawlResult, CrawlerConfig, CrawlerConfigBuilder, DiscoveredUrl,
    DownloadedAsset, ScrapedContent, ValidUrl,
};

// Application layer — Use cases
pub mod application;
pub use application::{
    crawl_site, create_http_client, discover_urls, extract_domain, fetch_sitemap, is_allowed,
    is_excluded, is_internal_link, matches_pattern, scrape_multiple_with_limit, scrape_with_config,
    scrape_with_readability,
};

// Infrastructure layer — Implementations (public for testing)
pub mod infrastructure;

// Adapters — External integrations (feature-gated)
pub mod adapters;

// Legacy re-exports for backward compatibility
pub mod extractor;
pub mod url_path;
pub mod user_agent;
pub use url_path::{Domain, OutputPath, UrlPath};

// CLI types
pub use clap::{Parser, ValueEnum};
pub use error::{Result, ScraperError};

/// Output format for scraped content
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Markdown format (recommended for RAG)
    Markdown,
    /// Plain text without formatting
    Text,
    /// Structured JSON
    Json,
}

/// Configuration for asset downloading
///
/// Following **config-externalize**: All concurrency settings are configurable.
/// Following **async-concurrency-limit**: Default is HDD-aware (3 for 4C CPU).
#[derive(Debug, Clone)]
pub struct ScraperConfig {
    /// Enable image downloading
    pub download_images: bool,
    /// Enable document downloading (PDF, DOCX, XLSX, etc.)
    pub download_documents: bool,
    /// Output directory for downloaded assets
    pub output_dir: std::path::PathBuf,
    /// Maximum file size in bytes (default: 50MB)
    pub max_file_size: Option<u64>,
    /// Maximum concurrent scrapers (default: 3 for HDD-aware on 4C CPU)
    /// Following rust-skills: config-externalize, async-concurrency-limit
    pub scraper_concurrency: usize,
}

impl Default for ScraperConfig {
    fn default() -> Self {
        Self {
            download_images: false,
            download_documents: false,
            output_dir: std::path::PathBuf::from("output"),
            max_file_size: Some(50 * 1024 * 1024), // 50MB default
            scraper_concurrency: 3,                // HDD-aware: nproc - 1 for 4C CPU
        }
    }
}

impl ScraperConfig {
    /// Create a new config with default values
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable image downloading
    #[must_use]
    pub fn with_images(mut self) -> Self {
        self.download_images = true;
        self
    }

    /// Enable document downloading
    #[must_use]
    pub fn with_documents(mut self) -> Self {
        self.download_documents = true;
        self
    }

    /// Set custom output directory
    #[must_use]
    pub fn with_output_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.output_dir = dir;
        self
    }

    /// Set scraper concurrency limit
    ///
    /// # Arguments
    ///
    /// * `concurrency` - Maximum concurrent scrapers
    ///
    /// # Recommendations
    ///
    /// - **HDD**: 3 (default) — avoids disk thrashing
    /// - **SSD**: 5-8 — faster random I/O
    /// - **NVMe**: 10+ — very high IOPS
    #[must_use]
    pub fn with_scraper_concurrency(mut self, concurrency: usize) -> Self {
        self.scraper_concurrency = concurrency;
        self
    }

    /// Check if any download is enabled
    pub fn has_downloads(&self) -> bool {
        self.download_images || self.download_documents
    }
}

/// CLI Arguments
#[derive(Parser, Debug)]
#[command(name = "rust-scraper")]
#[command(about = "Modern web scraper for RAG datasets", long_about = None)]
pub struct Args {
    /// URL to scrape (required)
    #[arg(short, long, required = true)]
    pub url: String,

    /// CSS selector (optional)
    #[arg(short, long, default_value = "body")]
    pub selector: String,

    /// Output directory
    #[arg(short, long, default_value = "output")]
    pub output: std::path::PathBuf,

    /// Output format
    #[arg(short, long, default_value = "markdown", value_enum)]
    pub format: OutputFormat,

    /// Delay between requests (ms)
    #[arg(long, default_value = "1000")]
    pub delay_ms: u64,

    /// Maximum pages to scrape
    #[arg(long, default_value = "10")]
    pub max_pages: usize,

    /// Download images from the page
    #[arg(long, default_value = "false")]
    pub download_images: bool,

    /// Download documents from the page
    #[arg(long, default_value = "false")]
    pub download_documents: bool,

    /// Verbosity level
    #[arg(short, long, action = clap::ArgAction::Count)]
    pub verbose: u8,
}

/// Validate and parse a URL
///
/// # Arguments
/// * `url` - URL string to validate
///
/// # Returns
/// * `Ok(url::Url)` - Validated and parsed URL
/// * `Err(ScraperError::InvalidUrl)` - Invalid URL
///
/// # Examples
///
/// ```
/// use rust_scraper::validate_and_parse_url;
///
/// let url = validate_and_parse_url("https://example.com").unwrap();
/// assert_eq!(url.host_str(), Some("example.com"));
///
/// let invalid = validate_and_parse_url("not-a-url");
/// assert!(invalid.is_err());
/// ```
pub fn validate_and_parse_url(url: &str) -> Result<url::Url> {
    if url.is_empty() {
        return Err(ScraperError::invalid_url("URL cannot be empty"));
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(ScraperError::invalid_url(
            "URL must start with http:// or https://",
        ));
    }

    let parsed = url::Url::parse(url)
        .map_err(|e| ScraperError::invalid_url(format!("Failed to parse URL: {}", e)))?;

    if parsed.host_str().is_none() {
        return Err(ScraperError::invalid_url("URL must have a valid host"));
    }

    Ok(parsed)
}

// Re-export save_results for convenience
pub use infrastructure::output::file_saver::save_results;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scraper_config_default() {
        let config = ScraperConfig::default();
        assert!(!config.download_images);
        assert!(!config.download_documents);
        assert!(!config.has_downloads());
        assert_eq!(config.scraper_concurrency, 3);
    }

    #[test]
    fn test_scraper_config_with_images() {
        let config = ScraperConfig::default().with_images();
        assert!(config.download_images);
        assert!(config.has_downloads());
    }

    #[test]
    fn test_scraper_config_with_documents() {
        let config = ScraperConfig::default().with_documents();
        assert!(config.download_documents);
        assert!(config.has_downloads());
    }

    #[test]
    fn test_scraper_config_with_concurrency() {
        let config = ScraperConfig::default().with_scraper_concurrency(5);
        assert_eq!(config.scraper_concurrency, 5);
    }

    #[test]
    fn test_validate_and_parse_url_success() {
        let url = validate_and_parse_url("https://example.com");
        assert!(url.is_ok());
    }

    #[test]
    fn test_validate_and_parse_url_empty() {
        let result = validate_and_parse_url("");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_and_parse_url_invalid_scheme() {
        let result = validate_and_parse_url("ftp://example.com");
        assert!(result.is_err());
    }
}
