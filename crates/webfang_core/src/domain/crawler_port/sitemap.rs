//! Sitemap port — domain-owned sitemap surface (VO, error type, parser trait).
//!
//! Not to be confused with `crate::domain::site::SitemapConfig`: that one
//! is the discovery-mode decision (off / auto / explicit `ValidUrl`), while
//! the `SitemapConfig` in the parent `crate::domain::crawler_port` module
//! carries the sitemap-*parser* pagination/batch knobs consumed by
//! `application::crawler::sitemap_discovery::build_sitemap_parser`.
//!
//! Extracted from `infrastructure::crawler::sitemap_parser` so
//! `application::crawler::sitemap_discovery` can depend on `domain::*`
//! without `application→infrastructure` (ADR-0012-B sitemap port, follow-up
//! of #1082). Infrastructure keeps the concrete `SitemapParser` (HTTP + XML
//! machinery) plus a `pub use` shim for the moved names — the repo's blessed
//! "move + shim" migration (ADR-0012 §6 tradeoff 2), the same shape as the
//! [`SitemapConfig`](crate::domain::crawler_port::SitemapConfig) precedent
//! in this module.
//!
//! # Async desugaring
//!
//! [`SitemapParserPort::parse_from_url`] uses manual `Pin<Box<dyn Future>>`
//! desugaring (`BoxFuture`) instead of the `async_trait` crate, matching the
//! frozen decision #1 established in
//! [`crate::domain::repository::VectorRepository`] and
//! [`crate::domain::downloader_port::Downloader`]. The trait stays
//! dyn-compatible: no generics, no `Self` in return position.
//!
//! # Error payload note
//!
//! [`SitemapError::XmlError`] carries the XML failure as a `String` rather
//! than `quick_xml::Error` (its former payload): `quick_xml` is an
//! infrastructure dependency and must not enter the domain. The stored
//! string is the `quick_xml::Error` `Display` text, so user-visible
//! messages stay byte-identical to the pre-port behavior.

use futures::future::BoxFuture;
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
    #[error("http request failed: {status}: {message}")]
    HttpError {
        /// HTTP status code
        status: u16,
        /// Error message
        message: String,
    },

    /// XML parsing of sitemap content failed
    ///
    /// Payload is the `quick_xml::Error` `Display` text (without the
    /// `"XML parsing failed: "` prefix, which the `#[error]` attribute adds)
    /// so the domain stays free of the `quick_xml` dependency.
    #[error("XML parsing failed: {0}")]
    XmlError(String),

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

    /// Decompression of compressed sitemap failed
    #[error("decompression failed: {0}")]
    DecompressionError(String),

    /// All child sitemaps in an index failed to parse
    #[error("all {0} child sitemaps failed: {1}")]
    AllChildrenFailed(usize, String),

    /// A WAF/CAPTCHA challenge page was served instead of sitemap content
    /// (issue #879). `provider` carries the formatted Spanish evidence chain.
    #[error("waf challenge detected at {url}: {provider}")]
    WafChallenge {
        /// URL that served the challenge page.
        url: String,
        /// Formatted Spanish evidence chain (REQ-WAF-08).
        provider: String,
    },
}

/// Sitemap URL entry with metadata per sitemaps.org spec
///
/// Includes optional `<lastmod>`, `<priority>`, and `<changefreq>` fields.
/// Following **api-common-traits**: implements Debug, Clone, PartialEq, Eq, Hash.
#[derive(Debug, Clone, PartialEq)]
pub struct SitemapUrl {
    /// The URL location (required)
    pub url: Url,
    /// Last modification date (RFC 3339 / W3C datetime)
    pub lastmod: Option<String>,
    /// Priority 0.0 - 1.0
    pub priority: Option<f32>,
    /// Change frequency: always, hourly, daily, weekly, monthly, yearly, never
    pub changefreq: Option<String>,
}

impl SitemapUrl {
    /// Create a new SitemapUrl with just the required URL field
    pub fn new(url: Url) -> Self {
        Self {
            url,
            lastmod: None,
            priority: None,
            changefreq: None,
        }
    }
}

/// Result type for sitemap operations
pub type Result<T> = std::result::Result<T, SitemapError>;

/// Domain port for sitemap parsing.
///
/// Implemented by [`crate::infrastructure::crawler::SitemapParser`] in
/// infrastructure; the application layer consumes it as
/// `Arc<dyn SitemapParserPort>` wired through the composition-root seam
/// `crate::application::container::build_sitemap_parser` — the only place
/// allowed to name the concrete (ADR-0012-B §2.1).
///
/// Dyn-compatible via `BoxFuture` (like
/// [`crate::domain::downloader_port::Downloader`]): single method, no
/// generics, no `Self` in return position. `Send + Sync` because sitemap
/// discovery polls it from `tokio::spawn`-ed crawl tasks on the
/// multi-threaded runtime.
pub trait SitemapParserPort: Send + Sync {
    /// Parse sitemap from URL (streaming, zero-allocation)
    ///
    /// # Arguments
    ///
    /// * `sitemap_url` - Sitemap URL (supports .xml and .xml.gz)
    ///
    /// # Returns
    ///
    /// Vector of valid URLs found in sitemap
    ///
    /// # Errors
    ///
    /// Returns `SitemapError` if parsing fails or no URLs found
    fn parse_from_url<'a>(&'a self, sitemap_url: &'a str)
        -> BoxFuture<'a, Result<Vec<SitemapUrl>>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn sitemap_url_new_defaults_metadata_to_none() {
        let url = Url::parse("https://example.com/page").unwrap();
        let entry = SitemapUrl::new(url.clone());
        assert_eq!(entry.url, url);
        assert_eq!(entry.lastmod, None);
        assert_eq!(entry.priority, None);
        assert_eq!(entry.changefreq, None);
    }

    #[test]
    fn sitemap_error_display_http_error() {
        let err = SitemapError::HttpError {
            status: 404,
            message: "server returned 404".to_string(),
        };
        assert_eq!(
            format!("{err}"),
            "http request failed: 404: server returned 404"
        );
    }

    /// The `XmlError` payload is the bare `quick_xml::Error` `Display` text;
    /// the `"XML parsing failed: "` prefix comes from the `#[error]`
    /// attribute — the same composition the pre-port
    /// `XmlError(quick_xml::Error)` produced.
    #[test]
    fn sitemap_error_display_xml_error_keeps_prefix() {
        let err = SitemapError::XmlError(
            "syntax error: tag not closed: `>` not found before end of input".to_string(),
        );
        assert_eq!(
            format!("{err}"),
            "XML parsing failed: syntax error: tag not closed: `>` not found before end of input"
        );
    }

    #[test]
    fn sitemap_error_display_no_urls_found() {
        assert_eq!(
            format!("{}", SitemapError::NoUrlsFound),
            "no URLs found in sitemap"
        );
    }

    /// Moved from the infrastructure `tests` module
    /// (`test_parse_sitemap_max_depth_exceeded`) together with the error type.
    #[test]
    fn sitemap_error_display_max_depth_exceeded() {
        let err = SitemapError::MaxDepthExceeded;
        assert_eq!(format!("{err}"), "maximum recursion depth exceeded");
    }

    /// Fake parser returning a canned URL list. Doubles as the
    /// dyn-compatibility compile test: if `SitemapParserPort` ever stops
    /// being object-safe (associated type, generic method, `Self` in return)
    /// the `Arc<dyn SitemapParserPort>` binding below fails to type-check.
    #[derive(Debug)]
    struct FakeSitemapParser {
        urls: Vec<SitemapUrl>,
    }

    impl SitemapParserPort for FakeSitemapParser {
        fn parse_from_url<'a>(
            &'a self,
            _sitemap_url: &'a str,
        ) -> BoxFuture<'a, Result<Vec<SitemapUrl>>> {
            Box::pin(async move { Ok(self.urls.clone()) })
        }
    }

    #[tokio::test]
    async fn fake_parser_returns_canned_urls_through_dyn_port() {
        let canned = vec![
            SitemapUrl::new(Url::parse("https://example.com/a").unwrap()),
            SitemapUrl::new(Url::parse("https://example.com/b").unwrap()),
        ];
        let parser: Arc<dyn SitemapParserPort> = Arc::new(FakeSitemapParser {
            urls: canned.clone(),
        });
        let urls = parser
            .parse_from_url("https://example.com/sitemap.xml")
            .await
            .unwrap();
        assert_eq!(urls, canned);
    }
}
