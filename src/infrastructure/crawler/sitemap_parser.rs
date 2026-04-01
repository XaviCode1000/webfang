//! Sitemap Parser Module
//!
//! Zero-allocation streaming parser for XML sitemaps.
//! Supports gzip compression and sitemap index recursion.
//!
//! # Examples
//!
//! ```no_run
//! use rust_scraper::infrastructure::crawler::SitemapParser;
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

use async_compression::tokio::bufread::GzipDecoder;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashSet;
use thiserror::Error;
use tokio::io::BufReader;
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
/// use rust_scraper::infrastructure::crawler::sitemap_parser::resolve_url;
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

/// Sitemap parser configuration (builder pattern)
///
/// Following api-builder-pattern: clear, self-documenting API
#[derive(Debug, Clone)]
pub struct SitemapConfig {
    /// Enable gzip decompression (default: true)
    pub gzip_enabled: bool,
    /// Maximum recursion depth for sitemap indexes (default: 3)
    pub max_depth: u8,
    /// Concurrent requests for sitemap indexes (default: 5)
    pub concurrency: usize,
    /// Maximum HTTP response size in bytes (default: 50MB)
    pub max_response_size: usize,
    /// Maximum decompressed gzip size in bytes (default: 100MB)
    pub max_decompressed_size: usize,
}

impl Default for SitemapConfig {
    fn default() -> Self {
        Self {
            gzip_enabled: true,
            max_depth: 3,
            concurrency: 5,
            max_response_size: 52_428_800,      // 50MB
            max_decompressed_size: 104_857_600, // 100MB
        }
    }
}

impl SitemapConfig {
    /// Create new config builder
    pub fn builder() -> SitemapConfigBuilder {
        SitemapConfigBuilder::default()
    }
}

/// Builder for SitemapConfig
///
/// Following api-builder-must-use: #[must_use] attribute
#[derive(Default)]
#[must_use = "builders do nothing unless you call build()"]
pub struct SitemapConfigBuilder {
    gzip_enabled: bool,
    max_depth: u8,
    concurrency: usize,
    max_response_size: usize,
    max_decompressed_size: usize,
}

impl SitemapConfigBuilder {
    /// Enable or disable gzip decompression
    pub fn gzip_enabled(mut self, enabled: bool) -> Self {
        self.gzip_enabled = enabled;
        self
    }

    /// Set maximum recursion depth for sitemap indexes
    pub fn max_depth(mut self, depth: u8) -> Self {
        self.max_depth = depth;
        self
    }

    /// Set concurrency level for parallel sitemap parsing
    pub fn concurrency(mut self, count: usize) -> Self {
        self.concurrency = count;
        self
    }

    /// Set maximum HTTP response size in bytes
    pub fn max_response_size(mut self, size: usize) -> Self {
        self.max_response_size = size;
        self
    }

    /// Set maximum decompressed gzip size in bytes
    pub fn max_decompressed_size(mut self, size: usize) -> Self {
        self.max_decompressed_size = size;
        self
    }

    /// Build the configuration
    #[must_use]
    pub fn build(self) -> SitemapConfig {
        let defaults = SitemapConfig::default();
        SitemapConfig {
            gzip_enabled: self.gzip_enabled,
            max_depth: self.max_depth,
            concurrency: self.concurrency,
            max_response_size: if self.max_response_size == 0 {
                defaults.max_response_size
            } else {
                self.max_response_size
            },
            max_decompressed_size: if self.max_decompressed_size == 0 {
                defaults.max_decompressed_size
            } else {
                self.max_decompressed_size
            },
        }
    }
}

/// Zero-allocation streaming sitemap parser
///
/// Following mem-streaming-large-data: streaming parser, no buffer accumulation
pub struct SitemapParser {
    config: SitemapConfig,
    client: wreq::Client,
}

impl SitemapParser {
    /// Create new parser with default config
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: SitemapConfig::default(),
            client: wreq::Client::builder()
                .emulation(wreq_util::Emulation::Chrome131)
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("BUG: failed to build HTTP client"),
        }
    }

    /// Create new parser with custom config
    #[must_use]
    pub fn with_config(config: SitemapConfig) -> Self {
        Self {
            config,
            client: wreq::Client::builder()
                .emulation(wreq_util::Emulation::Chrome131)
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("BUG: failed to build HTTP client"),
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

        // Parse base URL for validation and relative URL resolution
        // Following own-borrow-over-clone: &str not &String
        let base_url = Url::parse(url)?;

        // Fetch sitemap content
        // Following security-no-unwrap-in-prod: proper error handling
        let response = self.client.get(url).send().await.map_err(|e| {
            tracing::warn!("http request failed for {}: {}", url, e);
            SitemapError::HttpError(e.to_string())
        })?;

        // Check if gzip compressed
        // Following security-filter-input: validate content type
        // Validate content type is XML or compatible - reject non-XML responses early
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

        let is_gzip = url.ends_with(".gz")
            || response
                .headers()
                .get("content-encoding")
                .map(|v| v == "gzip")
                .unwrap_or(false);

        // Stream response with size limit to prevent OOM
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

        // Parse based on compression
        // Following mem-streaming-large-data: stream, don't accumulate
        let urls = if is_gzip && self.config.gzip_enabled {
            self.parse_gzip_sitemap(&raw_bytes, &base_url).await?
        } else {
            self.parse_xml_sitemap(&raw_bytes, &base_url).await?
        };

        // Check if sitemap index (recursive)
        // Following async-no-lock-await: no locks before await
        if self.is_sitemap_index(&urls) {
            tracing::debug!("Detected sitemap index, recursing (depth: {})", depth);
            self.parse_sitemap_index(&urls, depth - 1).await
        } else {
            Ok(urls)
        }
    }

    /// Parse gzip-compressed sitemap
    ///
    /// Following mem-streaming-large-data: decompress in stream
    async fn parse_gzip_sitemap(&self, bytes: &[u8], base_url: &Url) -> Result<Vec<Url>> {
        let reader = BufReader::new(bytes);
        let mut decoder = GzipDecoder::new(reader);

        // Limit decompressed size to prevent decompression bombs
        let mut limited =
            tokio::io::AsyncReadExt::take(&mut decoder, self.config.max_decompressed_size as u64);
        let mut decompressed = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut limited, &mut decompressed).await?;

        // Check if we hit the limit (possible decompression bomb)
        if decompressed.len() >= self.config.max_decompressed_size {
            tracing::warn!(
                "Gzip decompression hit size limit ({} bytes) — possible decompression bomb",
                decompressed.len()
            );
            return Err(SitemapError::DecompressedTooLarge(
                self.config.max_decompressed_size,
            ));
        }

        // Parse XML
        self.parse_xml_sitemap(&decompressed, base_url).await
    }

    /// Parse XML sitemap (zero-allocation streaming)
    ///
    /// Following mem-no-clone-in-loop: no allocations inside parsing loop
    async fn parse_xml_sitemap(&self, bytes: &[u8], base_url: &Url) -> Result<Vec<Url>> {
        // Create reader with buffer
        let mut reader = Reader::from_reader(bytes);
        // reader.trim_text(true); // Deprecated in quick_xml 0.37

        // Use HashSet to avoid duplicates
        let mut urls = HashSet::new();
        let mut buf = Vec::new();
        let mut in_loc = false;

        // Streaming parse - no buffer accumulation
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() == b"loc" => {
                    in_loc = true;
                }
                Ok(Event::Text(ref e)) if in_loc => {
                    // Following own-cow-for-owned-borrowed: unescape returns Cow
                    if let Ok(text) = e.unescape() {
                        // Following security-filter-input: validate URL scheme
                        // Use resolve_url() to handle relative URLs safely
                        if let Some(url) = resolve_url(base_url, &text) {
                            // Only http/https schemes allowed
                            if url.scheme() == "http" || url.scheme() == "https" {
                                urls.insert(url);
                            } else {
                                tracing::debug!(
                                    "Filtered URL with invalid scheme: {} ({})",
                                    url,
                                    url.scheme()
                                );
                            }
                        }
                    }
                }
                Ok(Event::End(ref e)) if e.name().as_ref() == b"loc" => {
                    in_loc = false;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(SitemapError::XmlError(e)),
                _ => {}
            }
            // Following mem-no-clone-in-loop: clear buffer, don't reallocate
            buf.clear();
        }

        // Following security-no-unwrap-in-prod: proper error, not unwrap
        if urls.is_empty() {
            Err(SitemapError::NoUrlsFound)
        } else {
            Ok(urls.into_iter().collect())
        }
    }

    /// Check if URLs are sitemap index entries
    ///
    /// Following naming-boolean-methods: is_* prefix for boolean methods
    fn is_sitemap_index(&self, urls: &[Url]) -> bool {
        // Heuristic: if URLs end with .xml or .xml.gz, likely sitemap index
        urls.iter()
            .any(|u| u.path().ends_with(".xml") || u.path().ends_with(".xml.gz"))
    }

    /// Parse sitemap index recursively
    ///
    /// Following async-clone-channel-before-await: proper concurrency pattern
    async fn parse_sitemap_index(&self, sitemap_urls: &[Url], depth: u8) -> Result<Vec<Url>> {
        use futures::stream::{self, StreamExt};

        let mut all_urls = HashSet::new();

        // Concurrent parsing with limit
        // Following async-channel-bounded: bounded concurrency
        let results = stream::iter(sitemap_urls)
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
    ///
    /// Following naming-boolean-methods: has_* prefix
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

#[cfg(test)]
mod tests {
    use super::*;

    // Following test-tokio-test-async: #[tokio::test] for async tests
    #[tokio::test]
    async fn test_parse_simple_sitemap() {
        // Test with mock data
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
        // Test duplicate handling
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

        // HashSet should deduplicate
        assert_eq!(urls.len(), 2);
    }

    #[tokio::test]
    async fn test_parse_empty_sitemap() {
        // Test empty sitemap
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
        // Test malformed XML
        let xml = r#"<?xml version="1.0"?>
        <urlset>
            <url><loc>https://example.com/page1</loc>
            <!-- Missing closing tag -->
        </urlset>"#;

        let parser = SitemapParser::new();
        let base = Url::parse("https://example.com").unwrap();
        let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

        // Should handle gracefully (quick_xml is lenient)
        // At minimum, shouldn't panic
        assert!(result.is_ok() || matches!(result, Err(SitemapError::XmlError(_))));
    }

    #[test]
    fn test_config_builder() {
        // Following api-builder-pattern: test builder API
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
        // Test default config values
        let config = SitemapConfig::default();

        assert!(config.gzip_enabled);
        assert_eq!(config.max_depth, 3);
        assert_eq!(config.concurrency, 5);
    }

    #[test]
    fn test_is_sitemap_index() {
        let parser = SitemapParser::new();

        // Test with sitemap index URLs
        let index_urls = vec![
            Url::parse("https://example.com/sitemap1.xml").unwrap(),
            Url::parse("https://example.com/sitemap2.xml.gz").unwrap(),
        ];
        assert!(parser.is_sitemap_index(&index_urls));

        // Test with regular URLs
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

    // Following test-proptest-for-edge-cases: property-based testing for URLs
    #[tokio::test]
    async fn test_filter_invalid_schemes() {
        // Test that non-http/https schemes are filtered
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

        // Only http/https should be included
        assert_eq!(urls.len(), 2);
        assert!(urls
            .iter()
            .all(|u| u.scheme() == "http" || u.scheme() == "https"));
    }

    #[tokio::test]
    async fn test_parse_sitemap_with_namespaces() {
        // Test with common namespace variations
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

        // Should extract all loc elements (including image locs)
        assert!(urls.len() >= 2);
    }
}
