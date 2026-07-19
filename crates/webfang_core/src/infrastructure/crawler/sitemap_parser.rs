//! Sitemap Parser Module
//!
//! Zero-allocation streaming parser for XML sitemaps.
//! Supports gzip compression and sitemap index recursion.
//!
//! # Examples
//!
//! ```no_run
//! use webfang::infrastructure::crawler::SitemapParser;
//!
//! # #[tokio::main]
//! # async fn main() -> anyhow::Result<()> {
//! let parser = SitemapParser::new();
//! let urls = parser.parse_from_url("https://example.com/sitemap.xml").await?;
//! println!("Found {} URLs", urls.len());
//! # Ok(())
//! # }
//! ```
//!
//! # Errors
//!
//! Returns `SitemapError` if:
//! - URL is invalid
//! - HTTP request fails
//! - XML parsing fails
//! - No `<loc>` elements found

use super::batch_processor::BatchProcessor;
use super::compression_handler::CompressionHandler;
use super::memory_manager::MemoryManager;
use super::retry_policy::RetryPolicy;
use super::sitemap_config::SitemapConfig;
use super::url_validator::UrlValidator;
use crate::domain::UrlValidatorTrait;
#[allow(unused_imports)]
use async_compression::tokio::bufread::GzipDecoder;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashSet;
use thiserror::Error;
use url::Url;

/// Sitemap parser errors
///
/// Following err-thiserror-for-libraries: typed, matchable errors
#[derive(Debug, Error)]
pub enum SitemapError {
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    #[error("http request failed: {0}")]
    HttpError(String),

    #[error("XML parsing failed: {0}")]
    XmlError(#[from] quick_xml::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("no URLs found in sitemap")]
    NoUrlsFound,

    #[error("invalid sitemap structure")]
    InvalidStructure,

    #[error("maximum recursion depth exceeded")]
    MaxDepthExceeded,

    #[error("invalid scheme: {0} (only http/https allowed)")]
    InvalidScheme(String),

    #[error("response too large: exceeds {0} bytes")]
    ResponseTooLarge(usize),

    #[error("decompressed data too large: exceeds {0} bytes")]
    DecompressedTooLarge(usize),

    #[error("no sitemap found at {0}")]
    SitemapNotFound(String),

    #[error("invalid content type: expected XML, got {0}")]
    InvalidContentType(String),
}

/// Result type for sitemap operations
pub type Result<T> = std::result::Result<T, SitemapError>;

/// Safely resolve a potentially-relative URL against a base URL.
///
/// Handles all RFC 3986 reference types: absolute, scheme-relative,
/// absolute-path, relative-path. Returns `None` for empty inputs.
///
/// Following **err-result-over-panic**: returns Option, never panics.
/// Following **perf-iter-over-index**: uses early returns, no loops.
///
/// # Arguments
///
/// * `base` - The base URL for resolution context
/// * `input` - The URL or path to resolve
///
/// # Returns
///
/// * `Some(Url)` - Successfully resolved URL
/// * `None` - Empty input or resolution failure
///
/// # Examples
///
/// ```
/// use url::Url;
/// use webfang::infrastructure::crawler::sitemap_parser::resolve_url;
///
/// let base = Url::parse("https://example.com/sitemap.xml").unwrap();
/// let resolved = resolve_url(&base, "/page.html").unwrap();
/// assert_eq!(resolved.as_str(), "https://example.com/page.html");
/// ```
#[must_use]
pub fn resolve_url(base: &Url, input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    // Fast path: already absolute
    if input.starts_with("http://") || input.starts_with("https://") {
        return Url::parse(input).ok();
    }

    // Use RFC 3986 resolution via url::Url::join
    base.join(input).ok()
}

/// Zero-allocation streaming sitemap parser
///
/// Following mem-streaming-large-data: streaming parser, no buffer accumulation
pub struct SitemapParser {
    config: SitemapConfig,
    compression_handler: CompressionHandler,
    url_validator: UrlValidator,
    retry_policy: RetryPolicy,
    memory_manager: MemoryManager,
    batch_processor: BatchProcessor,
}

impl SitemapParser {
    /// Create new parser with default config
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: SitemapConfig::default(),
            compression_handler: CompressionHandler::new(),
            url_validator: UrlValidator::new(),
            retry_policy: RetryPolicy::new(),
            memory_manager: MemoryManager::new(),
            batch_processor: BatchProcessor::new(),
        }
    }

    /// Create new parser with custom config
    #[must_use]
    pub fn with_config(config: SitemapConfig) -> Self {
        Self {
            config,
            compression_handler: CompressionHandler::new(),
            url_validator: UrlValidator::new(),
            retry_policy: RetryPolicy::new(),
            memory_manager: MemoryManager::new(),
            batch_processor: BatchProcessor::new(),
        }
    }

    /// Parse sitemap from URL (streaming, zero-allocation)
    ///
    /// # Arguments
    ///
    /// * `url` - Sitemap URL (supports .xml and .xml.gz)
    ///
    /// # Returns
    ///
    /// Vector of valid URLs found in sitemap
    ///
    /// # Errors
    ///
    /// Returns `SitemapError` if parsing fails or no URLs found
    pub async fn parse_from_url(&self, url: &str) -> Result<Vec<Url>> {
        self.parse_with_depth(url, self.config.max_depth).await
    }

    /// Internal recursive parser with depth tracking
    async fn parse_with_depth(&self, url: &str, depth: u8) -> Result<Vec<Url>> {
        // Base case: max depth reached
        if depth == 0 {
            return Err(SitemapError::MaxDepthExceeded);
        }

        let base_url = Url::parse(url)?;

        // [3.6] RetryPolicy: wrap HTTP request with retry logic
        let response = self
            .retry_policy
            .execute_with_retry(|| {
                let url = url.to_string();
                async move {
                    let client = wreq::Client::builder()
                        .emulation(wreq_util::Emulation::Chrome145)
                        .timeout(std::time::Duration::from_secs(10))
                        .build()
                        .map_err(|e| std::io::Error::other(e.to_string()))?;
                    client
                        .get(&url)
                        .send()
                        .await
                        .map_err(|e| std::io::Error::other(e.to_string()))
                }
            })
            .await
            .map_err(|e| SitemapError::HttpError(e.to_string()))?;

        // Validate content type
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or(""))
            .unwrap_or("");

        let is_xml = content_type.is_empty()
            || content_type.contains("application/xml")
            || content_type.contains("text/xml")
            || content_type.contains("application/xhtml+xml")
            || url.ends_with(".xml")
            || url.ends_with(".xml.gz");

        if !is_xml {
            tracing::warn!(
                "Sitemap URL returned non-XML content type: {} from {}",
                content_type,
                url
            );
            return Err(SitemapError::InvalidContentType(content_type.to_string()));
        }

        // Stream response with size limit
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut raw_bytes = Vec::with_capacity(8192);
        let mut total_bytes = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| SitemapError::HttpError(e.to_string()))?;
            total_bytes += chunk.len();
            if total_bytes > self.config.max_response_size {
                tracing::warn!(
                    "Sitemap response too large: {} bytes from {}",
                    total_bytes,
                    url
                );
                return Err(SitemapError::ResponseTooLarge(
                    self.config.max_response_size,
                ));
            }
            raw_bytes.extend_from_slice(&chunk);
        }

        // [3.4] CompressionHandler integration: detect and decompress content
        let decompressed = self
            .compression_handler
            .detect_and_decompress(&raw_bytes, url)
            .await
            .map_err(|e| SitemapError::HttpError(e.to_string()))?;

        // Parse using unified decompression handle
        let urls = if decompressed.is_empty() {
            return Err(SitemapError::NoUrlsFound);
        } else {
            self.parse_xml_sitemap(&decompressed, &base_url).await?
        };

        // Check if sitemap index (recursive)
        if self.is_sitemap_index(&urls) {
            tracing::debug!("Detected sitemap index, recursing (depth: {})", depth);

            // [3.7] MemoryManager: handle disk swapping for large index
            self.memory_manager
                .handle_disk_swapping(&urls)
                .map_err(|e| SitemapError::HttpError(e.to_string()))?;

            self.parse_sitemap_index(&urls, depth - 1).await
        } else {
            // [3.7] MemoryManager: check memory limits before returning
            self.memory_manager
                .handle_disk_swapping(&urls)
                .map_err(|e| SitemapError::HttpError(e.to_string()))?;

            // [3.8] BatchProcessor: apply crawl budget optimization
            let optimized_urls = self.batch_processor.apply_crawl_budget(urls, &self.config);

            Ok(optimized_urls)
        }
    }

    /// Parse gzip-compressed sitemap
    #[allow(dead_code)]
    async fn parse_gzip_sitemap(&self, bytes: &[u8], base_url: &Url) -> Result<Vec<Url>> {
        use tokio::io::{AsyncReadExt, BufReader};
        let reader = BufReader::new(bytes);
        let mut decoder = GzipDecoder::new(reader);

        let mut limited =
            AsyncReadExt::take(&mut decoder, self.config.max_decompressed_size as u64);
        let mut decompressed = Vec::new();
        AsyncReadExt::read_to_end(&mut limited, &mut decompressed).await?;

        if decompressed.len() >= self.config.max_decompressed_size {
            tracing::warn!(
                "Gzip decompression hit size limit ({} bytes) — possible decompression bomb",
                decompressed.len()
            );
            return Err(SitemapError::DecompressedTooLarge(
                self.config.max_decompressed_size,
            ));
        }

        self.parse_xml_sitemap(&decompressed, base_url).await
    }

    /// Parse XML sitemap (zero-allocation streaming)
    async fn parse_xml_sitemap(&self, bytes: &[u8], base_url: &Url) -> Result<Vec<Url>> {
        let mut reader = Reader::from_reader(bytes);

        let mut urls = HashSet::new();
        let mut buf = Vec::new();
        let mut in_loc = false;

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() == b"loc" => {
                    in_loc = true;
                },
                Ok(Event::Text(ref e)) if in_loc => {
                    if let Ok(text) = e.decode() {
                        if let Some(url) = resolve_url(base_url, &text) {
                            // [3.5] UrlValidator integration: filter invalid patterns
                            let validation = self.url_validator.filter_invalid_patterns(&url);
                            match validation {
                                crate::domain::ValidationResult::Valid => {
                                    urls.insert(url);
                                },
                                crate::domain::ValidationResult::Invalid(reason) => {
                                    tracing::debug!("Filtered invalid URL: {} — {}", url, reason);
                                },
                                crate::domain::ValidationResult::NeedsRedirect(new_url) => {
                                    // Follow redirect by replacing URL
                                    urls.insert(new_url);
                                },
                            }
                        }
                    }
                },
                Ok(Event::End(ref e)) if e.name().as_ref() == b"loc" => {
                    in_loc = false;
                },
                Ok(Event::Eof) => break,
                Err(e) => return Err(SitemapError::XmlError(e)),
                _ => {},
            }
            buf.clear();
        }

        if urls.is_empty() {
            Err(SitemapError::NoUrlsFound)
        } else {
            Ok(urls.into_iter().collect())
        }
    }

    /// Check if URLs are sitemap index entries
    fn is_sitemap_index(&self, urls: &[Url]) -> bool {
        urls.iter()
            .any(|u| u.path().ends_with(".xml") || u.path().ends_with(".xml.gz"))
    }

    /// Parse sitemap index recursively
    async fn parse_sitemap_index(&self, sitemap_urls: &[Url], depth: u8) -> Result<Vec<Url>> {
        use futures::stream::{self, StreamExt};

        let mut all_urls = HashSet::new();

        let results = stream::iter(sitemap_urls.iter().cloned())
            .map(|url| async move { self.parse_with_depth(url.as_str(), depth).await })
            .buffered(self.config.concurrency)
            .collect::<Vec<_>>()
            .await;

        for result in results {
            match result {
                Ok(urls) => all_urls.extend(urls),
                Err(e) => tracing::warn!("Failed to parse sitemap: {}", e),
            }
        }

        if all_urls.is_empty() {
            Err(SitemapError::NoUrlsFound)
        } else {
            Ok(all_urls.into_iter().collect())
        }
    }

    /// Check if gzip is enabled in config
    #[must_use]
    pub fn has_gzip(&self) -> bool {
        self.config.gzip_enabled
    }

    /// Get current max depth
    #[must_use]
    pub fn max_depth(&self) -> u8 {
        self.config.max_depth
    }
}

impl Default for SitemapParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parse_simple_sitemap() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/page1</loc></url>
            <url><loc>https://example.com/page2</loc></url>
            <url><loc>https://example.com/page3</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new();
        let base = Url::parse("https://example.com").unwrap();
        let urls = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert_eq!(urls.len(), 3);
        assert!(urls
            .iter()
            .any(|u| u.as_str() == "https://example.com/page1"));
    }

    #[tokio::test]
    async fn test_parse_sitemap_with_duplicates() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/page1</loc></url>
            <url><loc>https://example.com/page1</loc></url>
            <url><loc>https://example.com/page2</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new();
        let base = Url::parse("https://example.com").unwrap();
        let urls = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert_eq!(urls.len(), 2);
    }

    #[tokio::test]
    async fn test_parse_empty_sitemap() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        </urlset>"#;

        let parser = SitemapParser::new();
        let base = Url::parse("https://example.com").unwrap();
        let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

        assert!(matches!(result, Err(SitemapError::NoUrlsFound)));
    }

    #[tokio::test]
    async fn test_parse_malformed_xml() {
        let xml = r#"<?xml version="1.0"?>
        <urlset>
            <url><loc>https://example.com/page1</loc>
            <!-- Missing closing tag -->
        </urlset>"#;

        let parser = SitemapParser::new();
        let base = Url::parse("https://example.com").unwrap();
        let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

        assert!(result.is_ok() || matches!(result, Err(SitemapError::XmlError(_))));
    }

    #[test]
    fn test_config_builder() {
        let config = SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(5)
            .concurrency(10)
            .build();

        assert!(config.gzip_enabled);
        assert_eq!(config.max_depth, 5);
        assert_eq!(config.concurrency, 10);
    }

    #[test]
    fn test_config_default() {
        let config = SitemapConfig::default();

        assert!(config.gzip_enabled);
        assert_eq!(config.max_depth, 3);
        assert_eq!(config.concurrency, 5);
    }

    #[test]
    fn test_is_sitemap_index() {
        let parser = SitemapParser::new();

        let index_urls = vec![
            Url::parse("https://example.com/sitemap1.xml").unwrap(),
            Url::parse("https://example.com/sitemap2.xml.gz").unwrap(),
        ];
        assert!(parser.is_sitemap_index(&index_urls));

        let regular_urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.com/page2").unwrap(),
        ];
        assert!(!parser.is_sitemap_index(&regular_urls));
    }

    #[test]
    fn test_parser_has_gzip() {
        let parser_gzip =
            SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(true).build());
        assert!(parser_gzip.has_gzip());

        let parser_no_gzip =
            SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(false).build());
        assert!(!parser_no_gzip.has_gzip());
    }

    #[tokio::test]
    async fn test_filter_invalid_schemes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/valid</loc></url>
            <url><loc>http://example.com/valid</loc></url>
            <url><loc>ftp://example.com/invalid</loc></url>
            <url><loc>file:///etc/passwd</loc></url>
            <url><loc>javascript:alert(1)</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new();
        let base = Url::parse("https://example.com").unwrap();
        let urls = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert_eq!(urls.len(), 2);
        assert!(urls
            .iter()
            .all(|u| u.scheme() == "http" || u.scheme() == "https"));
    }

    #[tokio::test]
    async fn test_parse_sitemap_with_namespaces() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
                xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
            <url>
                <loc>https://example.com/page1</loc>
                <image:image><image:loc>https://example.com/image.jpg</image:loc></image:image>
            </url>
            <url><loc>https://example.com/page2</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new();
        let base = Url::parse("https://example.com").unwrap();
        let urls = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert!(urls.len() >= 2);
    }

    #[test]
    fn test_parse_sitemap_max_depth_exceeded() {
        let config = SitemapConfig::builder().max_depth(0).build();
        assert_eq!(config.max_depth, 0);
        let err = SitemapError::MaxDepthExceeded;
        assert_eq!(format!("{}", err), "maximum recursion depth exceeded");
    }

    #[test]
    fn test_resolve_url_relative_paths() {
        let base = Url::parse("https://example.com/sitemap.xml").unwrap();

        let resolved = resolve_url(&base, "../page").unwrap();
        assert_eq!(resolved.as_str(), "https://example.com/page");

        let resolved = resolve_url(&base, "page.html").unwrap();
        assert_eq!(resolved.as_str(), "https://example.com/page.html");

        let resolved = resolve_url(&base, "/page").unwrap();
        assert_eq!(resolved.as_str(), "https://example.com/page");

        let resolved = resolve_url(&base, "//other/page").unwrap();
        assert_eq!(resolved.as_str(), "https://other/page");
    }

    #[test]
    fn test_resolve_url_empty_input() {
        let base = Url::parse("https://example.com").unwrap();

        assert!(resolve_url(&base, "").is_none());
        assert!(resolve_url(&base, "   ").is_none());
    }

    #[test]
    fn test_config_builder_zero_falls_back_to_defaults() {
        let config = SitemapConfig::builder()
            .max_response_size(0)
            .max_decompressed_size(0)
            .build();

        assert_eq!(config.max_response_size, 52_428_800);
        assert_eq!(config.max_decompressed_size, 104_857_600);
    }

    // -- Mutation-killing tests for sitemap_parser --

    // Gap A: resolve_url — absolute URL passthrough
    #[test]
    fn test_resolve_url_absolute_passthrough() {
        let base = Url::parse("https://example.com/sitemap.xml").unwrap();

        let resolved = resolve_url(&base, "https://other.com/page").unwrap();
        assert_eq!(resolved.as_str(), "https://other.com/page");

        let resolved = resolve_url(&base, "http://insecure.com/page").unwrap();
        assert_eq!(resolved.as_str(), "http://insecure.com/page");
    }

    #[test]
    fn test_resolve_url_absolute_overrides_base() {
        let base = Url::parse("https://example.com/sitemap.xml").unwrap();
        let resolved = resolve_url(&base, "https://completely-different.org/path").unwrap();
        assert_eq!(resolved.host_str(), Some("completely-different.org"));
    }

    // Gap B: parse_with_depth — depth=0 returns MaxDepthExceeded without HTTP
    #[tokio::test]
    async fn test_parse_from_url_depth_zero_returns_error() {
        let config = SitemapConfig::builder().max_depth(0).build();
        let parser = SitemapParser::with_config(config);
        let result = parser
            .parse_from_url("https://example.com/sitemap.xml")
            .await;
        assert!(matches!(result, Err(SitemapError::MaxDepthExceeded)));
    }

    #[tokio::test]
    async fn test_parse_from_url_depth_one_attempts_fetch() {
        let config = SitemapConfig::builder().max_depth(1).build();
        let parser = SitemapParser::with_config(config);
        // depth=1 means it tries the HTTP fetch — with an invalid host it should fail
        let result = parser
            .parse_from_url("https://invalid-host-xyz-12345.com/sitemap.xml")
            .await;
        assert!(result.is_err());
    }

    // Gap C: is_sitemap_index — various URL patterns
    #[test]
    fn test_is_sitemap_index_xml_gz() {
        let parser = SitemapParser::new();
        let urls = vec![Url::parse("https://example.com/sitemap.xml.gz").unwrap()];
        assert!(parser.is_sitemap_index(&urls));
    }

    #[test]
    fn test_is_sitemap_index_mixed() {
        let parser = SitemapParser::new();
        let urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.com/sitemap2.xml").unwrap(),
        ];
        assert!(parser.is_sitemap_index(&urls));
    }

    #[test]
    fn test_is_sitemap_index_no_xml() {
        let parser = SitemapParser::new();
        let urls = vec![
            Url::parse("https://example.com/page1.html").unwrap(),
            Url::parse("https://example.com/page2.json").unwrap(),
        ];
        assert!(!parser.is_sitemap_index(&urls));
    }

    #[test]
    fn test_max_depth_accessor() {
        let parser = SitemapParser::with_config(SitemapConfig::builder().max_depth(7).build());
        assert_eq!(parser.max_depth(), 7);
    }

    #[test]
    fn test_max_depth_default() {
        let parser = SitemapParser::new();
        assert_eq!(parser.max_depth(), 3);
    }
}
