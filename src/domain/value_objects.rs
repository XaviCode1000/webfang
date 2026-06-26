//! Value objects — Type-safe primitives
//!
//! Value objects are immutable types that are defined by their attributes,
//! not by identity. They provide type safety at compile time.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// W3C TraceContext CorrelationId value object
///
/// Provides W3C-compliant traceparent headers for distributed tracing.
/// Generates UUID v7 for the trace_id (timestamp + random) and
/// a random span_id for span identification.
///
/// # W3C Traceparent Format
///
/// `00-{trace_id}-{span_id}-{trace_flags}`
/// - trace_id: 32-character lowercase hex (UUID v7)
/// - span_id: 16-character lowercase hex
/// - trace_flags: 01 (sampled)
///
/// # Examples
///
/// ```
/// use rust_scraper::domain::value_objects::CorrelationId;
///
/// let correlation_id = CorrelationId::new();
/// let traceparent = correlation_id.to_traceparent();
/// assert!(traceparent.starts_with("00-"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationId {
    /// 128-bit UUID v7 (timestamp + random)
    trace_id: Uuid,
    /// 64-bit random span identifier
    span_id: u64,
}

impl CorrelationId {
    /// Create a new CorrelationId with fresh UUID v7 and random span_id
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::domain::value_objects::CorrelationId;
    ///
    /// let corr = CorrelationId::new();
    /// let traceparent = corr.to_traceparent();
    /// assert!(traceparent.starts_with("00-"));
    /// ```
    pub fn new() -> Self {
        use rand::Rng;
        let mut rng = rand::rng();
        Self {
            trace_id: Uuid::now_v7(),
            span_id: rng.random(),
        }
    }

    /// Create CorrelationId from existing trace_id and span_id
    ///
    /// Useful for propagating existing correlation IDs through the system.
    pub fn new_with_ids(trace_id: Uuid, span_id: u64) -> Self {
        Self { trace_id, span_id }
    }

    /// Generate W3C traceparent header value
    ///
    /// Returns format: `00-{trace_id}-{span_id}-01`
    /// - trace_id: 32-character lowercase hex
    /// - span_id: 16-character lowercase hex
    /// - trace_flags: 01 (sampled)
    pub fn to_traceparent(&self) -> String {
        format!(
            "00-{:032x}-{:016x}-01",
            self.trace_id.as_u128(),
            self.span_id
        )
    }

    /// Get the trace_id as Uuid
    pub fn trace_id(&self) -> Uuid {
        self.trace_id
    }

    /// Get the span_id
    pub fn span_id(&self) -> u64 {
        self.span_id
    }

    /// Generate W3C tracestate header value
    ///
    /// Returns `rust_scraper=v1:{trace_id}` vendor entry format.
    pub fn to_tracestate(&self) -> String {
        format!("rust_scraper=v1:{:032x}", self.trace_id.as_u128())
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_traceparent())
    }
}

/// Validated URL newtype - guarantees URL is valid at type level
///
/// This enforces that ScrapedContent always has a valid URL,
/// preventing runtime errors from invalid URLs.
///
/// # Examples
///
/// ```
/// use rust_scraper::domain::ValidUrl;
///
/// // Create from parsed URL
/// let url = url::Url::parse("https://example.com").unwrap();
/// let valid = ValidUrl::new(url);
/// assert_eq!(valid.as_str(), "https://example.com/");  // URL adds trailing slash
///
/// // Or parse directly
/// let valid = ValidUrl::parse("https://example.com").unwrap();
/// assert!(valid.as_str().starts_with("https://example.com"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidUrl(url::Url);

impl ValidUrl {
    /// Create a new ValidUrl from a validated url::Url
    ///
    /// This is infallible since the URL is already parsed.
    pub fn new(url: url::Url) -> Self {
        Self(url)
    }

    /// Parse and create a ValidUrl from a string
    ///
    /// Returns error if the string is not a valid URL.
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::domain::ValidUrl;
    ///
    /// let url = ValidUrl::parse("https://example.com").unwrap();
    /// assert_eq!(url.host_str(), Some("example.com"));
    ///
    /// let invalid = ValidUrl::parse("not-a-url");
    /// assert!(invalid.is_err());
    /// ```
    pub fn parse(s: &str) -> crate::Result<Self> {
        Ok(Self(url::Url::parse(s).map_err(|e| {
            crate::ScraperError::invalid_url(e.to_string())
        })?))
    }

    /// Get reference to inner url::Url
    pub fn as_url(&self) -> &url::Url {
        &self.0
    }

    /// Get the URL as string
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Get the host portion of the URL
    pub fn host_str(&self) -> Option<&str> {
        self.0.host_str()
    }

    /// Get the scheme (protocol) of the URL
    pub fn scheme(&self) -> &str {
        self.0.scheme()
    }

    /// Get the path portion of the URL
    pub fn path(&self) -> &str {
        self.0.path()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_url_new() {
        let url = url::Url::parse("https://example.com").unwrap();
        let valid = ValidUrl::new(url);
        assert_eq!(valid.as_str(), "https://example.com/"); // URL adds trailing slash
    }

    #[test]
    fn test_valid_url_parse_success() {
        let valid = ValidUrl::parse("https://example.com/article");
        assert!(valid.is_ok());
        let valid = valid.unwrap();
        assert_eq!(valid.host_str(), Some("example.com"));
        assert_eq!(valid.path(), "/article");
    }

    #[test]
    fn test_valid_url_parse_invalid() {
        let result = ValidUrl::parse("not-a-url");
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_url_from_trait() {
        let url = url::Url::parse("https://example.com").unwrap();
        let valid: ValidUrl = url.into();
        assert_eq!(valid.as_str(), "https://example.com/"); // URL adds trailing slash
    }

    #[test]
    fn test_valid_url_display() {
        let url = ValidUrl::parse("https://example.com").unwrap();
        assert_eq!(format!("{}", url), "https://example.com/");
    }

    #[test]
    fn test_valid_url_with_query() {
        let url = ValidUrl::parse("https://example.com/search?q=rust").unwrap();
        assert_eq!(url.as_url().query(), Some("q=rust"));
    }

    #[test]
    fn test_valid_url_with_port() {
        let url = ValidUrl::parse("http://localhost:8080/api").unwrap();
        assert_eq!(url.host_str(), Some("localhost"));
        assert_eq!(url.as_url().port(), Some(8080));
    }

    // ========================================================================
    // CorrelationId tests
    // ========================================================================

    #[test]
    fn test_correlation_id_new_generates_valid_ids() {
        let corr = CorrelationId::new();

        // trace_id should be a valid UUID v7 (version byte = 7)
        let trace_id = corr.trace_id();
        let uuid_bytes = trace_id.as_bytes();
        let version_nibble = (uuid_bytes[6] >> 4) & 0x0F;
        assert_eq!(version_nibble, 7, "trace_id should be UUID v7");

        // span_id should be non-zero (random u64)
        assert!(corr.span_id() != 0, "span_id should be non-zero");
    }

    #[test]
    fn test_correlation_id_to_traceparent_format() {
        let corr = CorrelationId::new();
        let traceparent = corr.to_traceparent();

        // Format: 00-{32 hex trace_id}-{16 hex span_id}-01
        // Total length: 2 + 1 + 32 + 1 + 16 + 1 + 2 = 55
        assert_eq!(traceparent.len(), 55, "traceparent should be 55 chars");
        assert!(traceparent.starts_with("00-"), "should start with 00-");
        assert!(traceparent.ends_with("-01"), "should end with -01");

        // Middle sections should be valid hex
        let parts: Vec<&str> = traceparent.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[1].len(), 32, "trace_id should be 32 hex chars");
        assert_eq!(parts[2].len(), 16, "span_id should be 16 hex chars");
    }

    #[test]
    fn test_correlation_id_clone_is_identical() {
        let corr = CorrelationId::new();
        let cloned = corr.clone();

        assert_eq!(corr.trace_id(), cloned.trace_id());
        assert_eq!(corr.span_id(), cloned.span_id());
        assert_eq!(corr.to_traceparent(), cloned.to_traceparent());
    }

    #[test]
    fn test_correlation_id_send_sync() {
        // Compile-time check: CorrelationId is Send + Sync
        fn _check_send_sync<T: Send + Sync>(_: &T) {}

        let corr = CorrelationId::new();
        _check_send_sync(&corr);
    }

    #[test]
    fn test_correlation_id_display() {
        let corr = CorrelationId::new();
        let display = format!("{}", corr);

        assert!(display.starts_with("00-"));
        assert_eq!(display, corr.to_traceparent());
    }

    #[test]
    fn test_correlation_id_tracestate() {
        let corr = CorrelationId::new();
        let tracestate = corr.to_tracestate();

        // Format: rust_scraper=v1:{32 hex trace_id}
        // rust_scraper=v1: = 16 chars
        // trace_id = 32 chars
        // Total = 48 chars
        assert!(tracestate.starts_with("rust_scraper=v1:"));
        assert!(tracestate.contains('='));
        assert_eq!(tracestate.len(), 48);
    }
}
