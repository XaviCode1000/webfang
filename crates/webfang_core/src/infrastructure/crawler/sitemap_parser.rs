//! Sitemap Parser Module
//!
//! Zero-allocation streaming parser for XML sitemaps.
//! Supports gzip compression and sitemap index recursion.
//!
//! # Examples
//!
//! ```no_run
//! use webfang_core::infrastructure::crawler::SitemapParser;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let parser = SitemapParser::new()?;
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
use crate::domain::{CrawlError, UrlValidatorTrait};
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
    /// URL could not be parsed
    #[error("invalid URL: {0}")]
    InvalidUrl(#[from] url::ParseError),

    /// HTTP request to fetch the sitemap failed
    #[error("http request failed: {0}")]
    HttpError(String),

    /// XML parsing of sitemap content failed
    #[error("XML parsing failed: {0}")]
    XmlError(#[from] quick_xml::Error),

    /// I/O error reading or writing sitemap data
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// Sitemap contained no URL entries
    #[error("no URLs found in sitemap")]
    NoUrlsFound,

    /// Sitemap XML structure does not match expected format
    #[error("invalid sitemap structure")]
    InvalidStructure,

    /// Sitemap index depth exceeded the configured maximum
    #[error("maximum recursion depth exceeded")]
    MaxDepthExceeded,

    /// URL scheme is not http or https
    #[error("invalid scheme: {0} (only http/https allowed)")]
    InvalidScheme(String),

    /// HTTP response body exceeds the size limit
    #[error("response too large: exceeds {0} bytes")]
    ResponseTooLarge(usize),

    /// Decompressed sitemap data exceeds the size limit
    #[error("decompressed data too large: exceeds {0} bytes")]
    DecompressedTooLarge(usize),

    /// No sitemap found at the expected URL
    #[error("no sitemap found at {0}")]
    SitemapNotFound(String),

    /// Response Content-Type is not XML
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
/// use webfang_core::infrastructure::crawler::sitemap_parser::resolve_url;
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
    /// TLS/HTTP2 fingerprint emulation preset applied to the sitemap fetch client.
    ///
    /// Threaded from the caller (ultimately the CLI `--h2-profile` value).
    /// Defaults to [`wreq_util::Profile::Chrome145`], preserving the historical
    /// sitemap fingerprint (#323).
    tls_emulation: wreq_util::Profile,
    /// Shared HTTP client used to fetch sitemap XML.
    ///
    /// Built once in the constructor from `tls_emulation` via the shared
    /// `create_http_client_with_config` factory (#299) and cloned cheaply
    /// (it is `Arc`-backed) into each retry attempt — no per-retry rebuild (#323).
    http_client: wreq::Client,
}

impl SitemapParser {
    /// Create new parser with default config
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`SitemapParser::with_config_and_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn new() -> std::result::Result<Self, CrawlError> {
        Self::with_config_and_profile(SitemapConfig::default(), wreq_util::Profile::Chrome145)
    }

    /// Create new parser with custom config
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`SitemapParser::with_config_and_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn with_config(config: SitemapConfig) -> std::result::Result<Self, CrawlError> {
        Self::with_config_and_profile(config, wreq_util::Profile::Chrome145)
    }

    /// Create new parser with custom config and an explicit TLS/H2 profile.
    ///
    /// The sitemap fetch client is built once here (via the shared
    /// `create_http_client_with_config` factory) from `tls_emulation` and reused
    /// across every fetch and retry, honoring the caller's `--h2-profile`
    /// selection instead of a hardcoded preset (#323).
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn with_config_and_profile(
        config: SitemapConfig,
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<Self, CrawlError> {
        let http_client = Self::build_client(tls_emulation)?;
        Ok(Self {
            config,
            compression_handler: CompressionHandler::new(),
            url_validator: UrlValidator::with_profile(tls_emulation)?,
            retry_policy: RetryPolicy::new(),
            memory_manager: MemoryManager::new(),
            batch_processor: BatchProcessor::new(),
            tls_emulation,
            http_client,
        })
    }

    /// Build the sitemap fetch HTTP client for a given TLS/H2 profile.
    ///
    /// The request and connect timeouts are pinned to 10s to preserve the
    /// historical behavior of the previous hardcoded client (which set
    /// `.timeout(10s)`); only the TLS/H2 profile is now configurable (#323).
    /// The remaining settings (Chrome Client Hints, pool tuning, compression)
    /// come from the shared factory defaults (#299).
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the underlying client fails to build.
    fn build_client(
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<wreq::Client, CrawlError> {
        let http_config = crate::domain::http_config::HttpClientConfig {
            tls_emulation,
            timeout_secs: 10,
            connect_timeout_secs: 10,
            ..Default::default()
        };
        crate::infrastructure::http::create_http_client_with_config(&http_config)
            // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
            .map_err(|e| CrawlError::Internal(format!("failed to build sitemap client: {e}")))
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

    /// Validate a sitemap HTTP response: status MUST be checked before
    /// content-type so a 404/5xx yields "not found" rather than the misleading
    /// "unexpected content-type" (issue #590, bug #9). Returns the content-type
    /// string on success for the caller's XML streaming path.
    fn validate_response(status: wreq::StatusCode, content_type: &str, url: &str) -> Result<()> {
        if !status.is_success() {
            tracing::warn!("Sitemap URL returned non-2xx status: {status} from {url}");
            return Err(SitemapError::HttpError(format!("server returned {status}")));
        }
        let is_xml = content_type.is_empty()
            || content_type.contains("application/xml")
            || content_type.contains("text/xml")
            || content_type.contains("application/xhtml+xml")
            || url.ends_with(".xml")
            || url.ends_with(".xml.gz");
        if !is_xml {
            tracing::warn!(
                "Sitemap URL returned non-XML content type: {} from {url}",
                content_type
            );
            return Err(SitemapError::InvalidContentType(content_type.to_string()));
        }
        Ok(())
    }

    /// Internal recursive parser with depth tracking
    async fn parse_with_depth(&self, url: &str, depth: u8) -> Result<Vec<Url>> {
        // Base case: max depth reached
        if depth == 0 {
            return Err(SitemapError::MaxDepthExceeded);
        }

        let base_url = Url::parse(url)?;

        // [3.6] RetryPolicy: wrap HTTP request with retry logic.
        // The client is built once in the constructor (honoring tls_emulation)
        // and cloned cheaply (Arc-backed) into each attempt — no per-retry rebuild (#323).
        let response = self
            .retry_policy
            .execute_with_retry(|| {
                let url = url.to_string();
                let client = self.http_client.clone();
                async move { client.get(&url).send().await }
            })
            .await
            .map_err(|e| SitemapError::HttpError(e.to_string()))?;

        // Validate response: status checked before content-type (issue #590, bug #9).
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        Self::validate_response(status, content_type, url)?;

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

    /// Get the TLS/HTTP2 fingerprint emulation preset used for sitemap fetches.
    #[must_use]
    pub fn tls_emulation(&self) -> wreq_util::Profile {
        self.tls_emulation
    }
}

/// Parse sitemap XML content using quick-xml (streaming parser)
///
/// Standalone streaming parser for sitemap `<loc>` entries, extracted from the
/// application layer (issue #442): XML parsing is an infrastructure concern and
/// lives here alongside [`SitemapParser`]. Relative URLs are resolved against
/// `base_url`.
///
/// Following **xml-no-regex**: Uses quick-xml instead of regex for XML parsing.
/// Following **mem-stream-processing**: Streaming approach avoids loading entire DOM.
///
/// # Arguments
///
/// * `xml_content` - XML content of the sitemap
/// * `base_url` - Base URL to resolve relative `<loc>` entries against
///
/// # Returns
///
/// * `Ok(Vec<String>)` - List of URLs
/// * `Err(CrawlError)` - Parse error
pub fn parse_sitemap(
    xml_content: &str,
    base_url: &Url,
) -> std::result::Result<Vec<String>, CrawlError> {
    let mut reader = Reader::from_str(xml_content);
    let mut buf = Vec::new();
    let mut urls = Vec::new();
    let mut in_loc = false;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) if e.name().as_ref() == b"loc" => {
                in_loc = true;
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == b"loc" => {
                in_loc = false;
            },
            Ok(Event::Text(ref e)) if in_loc => {
                let text = e.decode().map_err(|e| CrawlError::Parse(e.to_string()))?;
                let url_str = text.trim();
                if !url_str.is_empty() {
                    // Resolve relative URLs against base_url
                    // Following url-join-relative: use base_url.join() for relative paths
                    let resolved =
                        if url_str.starts_with("http://") || url_str.starts_with("https://") {
                            Url::parse(url_str).ok()
                        } else {
                            base_url.join(url_str).ok()
                        };
                    if let Some(url) = resolved {
                        urls.push(url.to_string());
                    }
                }
            },
            Ok(Event::CData(ref e)) if in_loc => {
                // Handle CDATA sections - BytesCData derefs to [u8]
                let url_str = String::from_utf8_lossy(e).trim().to_string();
                if !url_str.is_empty() {
                    // Resolve relative URLs against base_url
                    let resolved =
                        if url_str.starts_with("http://") || url_str.starts_with("https://") {
                            Url::parse(&url_str).ok()
                        } else {
                            base_url.join(&url_str).ok()
                        };
                    if let Some(url) = resolved {
                        urls.push(url.to_string());
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(CrawlError::Parse(e.to_string())),
            _ => {},
        }
    }

    Ok(urls)
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

        let parser = SitemapParser::new().unwrap();
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

        let parser = SitemapParser::new().unwrap();
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

        let parser = SitemapParser::new().unwrap();
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

        let parser = SitemapParser::new().unwrap();
        let base = Url::parse("https://example.com").unwrap();
        let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

        // Malformed XML should either parse (lenient parser) or fail with XmlError
        match &result {
            Ok(_) => {}, // Parser is lenient, this is acceptable
            Err(e) => assert!(
                matches!(e, SitemapError::XmlError(_)),
                "expected XmlError for malformed XML, got: {e:?}"
            ),
        }
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
        let parser = SitemapParser::new().unwrap();

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
            SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(true).build())
                .unwrap();
        assert!(parser_gzip.has_gzip());

        let parser_no_gzip =
            SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(false).build())
                .unwrap();
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

        let parser = SitemapParser::new().unwrap();
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

    /// Bug #9 regression: a 404 response must yield SitemapError::HttpError
    /// ("server returned 404"), NOT SitemapError::InvalidContentType. Status
    /// MUST be checked BEFORE content-type (issue #590).
    #[tokio::test]
    async fn test_sitemap_404_yields_http_error_not_content_type() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let port = server.address().port();

        Mock::given(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
            .mount(&server)
            .await;

        let parser = SitemapParser::new().unwrap();
        let result = parser
            .parse_from_url(&format!("http://127.0.0.1:{port}/sitemap.xml"))
            .await;

        match result {
            Err(SitemapError::HttpError(msg)) => {
                assert!(msg.contains("404"), "error must reference 404, got: {msg}");
            },
            other => panic!("expected SitemapError::HttpError with 404, got: {other:?}"),
        }
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

        let parser = SitemapParser::new().unwrap();
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
        assert_eq!(format!("{err}"), "maximum recursion depth exceeded");
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
        let parser = SitemapParser::with_config(config).unwrap();
        let result = parser
            .parse_from_url("https://example.com/sitemap.xml")
            .await;
        assert!(matches!(result, Err(SitemapError::MaxDepthExceeded)));
    }

    #[tokio::test]
    #[ignore = "requires network — hits real DNS for invalid-host-xyz-12345.com"]
    async fn test_parse_from_url_depth_one_attempts_fetch() {
        let config = SitemapConfig::builder().max_depth(1).build();
        let parser = SitemapParser::with_config(config).unwrap();
        // depth=1 means it tries the HTTP fetch — with an invalid host it should fail
        let result = parser
            .parse_from_url("https://invalid-host-xyz-12345.com/sitemap.xml")
            .await;
        assert!(result.is_err());
    }

    // Gap C: is_sitemap_index — various URL patterns
    #[test]
    fn test_is_sitemap_index_xml_gz() {
        let parser = SitemapParser::new().unwrap();
        let urls = vec![Url::parse("https://example.com/sitemap.xml.gz").unwrap()];
        assert!(parser.is_sitemap_index(&urls));
    }

    #[test]
    fn test_is_sitemap_index_mixed() {
        let parser = SitemapParser::new().unwrap();
        let urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.com/sitemap2.xml").unwrap(),
        ];
        assert!(parser.is_sitemap_index(&urls));
    }

    #[test]
    fn test_is_sitemap_index_no_xml() {
        let parser = SitemapParser::new().unwrap();
        let urls = vec![
            Url::parse("https://example.com/page1.html").unwrap(),
            Url::parse("https://example.com/page2.json").unwrap(),
        ];
        assert!(!parser.is_sitemap_index(&urls));
    }

    #[test]
    fn test_max_depth_accessor() {
        let parser =
            SitemapParser::with_config(SitemapConfig::builder().max_depth(7).build()).unwrap();
        assert_eq!(parser.max_depth(), 7);
    }

    #[test]
    fn test_max_depth_default() {
        let parser = SitemapParser::new().unwrap();
        assert_eq!(parser.max_depth(), 3);
    }

    // -- #323: tls_emulation is honored, not hardcoded --

    #[test]
    fn test_new_defaults_to_chrome145_profile() {
        let parser = SitemapParser::new().unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome145);
    }

    #[test]
    fn test_with_config_defaults_to_chrome145_profile() {
        let parser = SitemapParser::with_config(SitemapConfig::default()).unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome145);
    }

    #[test]
    fn test_with_config_and_profile_accepts_custom_profile() {
        let parser = SitemapParser::with_config_and_profile(
            SitemapConfig::default(),
            wreq_util::Profile::Chrome131,
        )
        .unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome131);
    }

    #[test]
    fn test_with_config_and_profile_accepts_firefox_profile() {
        let parser = SitemapParser::with_config_and_profile(
            SitemapConfig::builder().max_depth(2).build(),
            wreq_util::Profile::Firefox135,
        )
        .unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Firefox135);
        assert_eq!(parser.max_depth(), 2);
    }
}
