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
/// use webfang_core::domain::value_objects::CorrelationId;
///
/// let correlation_id = CorrelationId::new();
/// let traceparent = correlation_id.to_traceparent();
/// assert!(traceparent.starts_with("00-"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// use webfang_core::domain::value_objects::CorrelationId;
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

    /// Create a child correlation ID: same `trace_id`, fresh `span_id`.
    ///
    /// Used to correlate multiple operations under a single trace while
    /// giving each its own span identity — e.g. every page of a crawl shares
    /// the crawl's `trace_id` but gets a unique `span_id`, so the whole crawl
    /// can be reconstructed by `trace_id` while each page stays distinguishable.
    ///
    /// # Examples
    ///
    /// ```
    /// use webfang_core::domain::value_objects::CorrelationId;
    ///
    /// let crawl = CorrelationId::new();
    /// let page = crawl.child();
    /// assert_eq!(crawl.trace_id(), page.trace_id());
    /// assert_ne!(crawl.span_id(), page.span_id());
    /// ```
    pub fn child(&self) -> Self {
        use rand::Rng;
        let mut rng = rand::rng();
        Self {
            trace_id: self.trace_id,
            span_id: rng.random(),
        }
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
    /// Returns `webfang=v1:{trace_id}` vendor entry format.
    pub fn to_tracestate(&self) -> String {
        format!("webfang=v1:{:032x}", self.trace_id.as_u128())
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
/// use webfang_core::domain::ValidUrl;
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// use webfang_core::domain::ValidUrl;
    ///
    /// let url = ValidUrl::parse("https://example.com").unwrap();
    /// assert_eq!(url.host_str(), Some("example.com"));
    ///
    /// let invalid = ValidUrl::parse("not-a-url");
    /// assert!(invalid.is_err());
    /// ```
    pub fn parse(s: &str) -> crate::Result<Self> {
        let parsed =
            url::Url::parse(s).map_err(|e| crate::ScraperError::invalid_url(e.to_string()))?;
        Self::try_from_url(parsed)
    }

    /// Apply the URL hardening policy to an ALREADY-parsed `url::Url`
    /// (#1117) — the single home of the #675-2 scheme allow-list and the
    /// #675-5 credential strip, shared with [`parse`](Self::parse).
    ///
    /// For callers like the asset extractor, whose URLs come from
    /// `base.join(src)` (a hostile `src` can change the scheme), this is
    /// the validating edge that keeps the policy in ONE place instead of
    /// re-implementing it per call site. An inherent method rather than
    /// `TryFrom<url::Url>` because the existing infallible
    /// `From<url::Url>` impl already owns the blanket `TryFrom`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::InvalidUrl`] for any non-http(s) scheme.
    pub fn try_from_url(url: url::Url) -> crate::Result<Self> {
        let mut url = url;
        // Bug #675-2: reject non-HTTP(S) schemes early (fail-fast).
        // `url::Url` accepts WHATWG-valid schemes like `data:`, `blob:`,
        // `file:`, `ftp:` — none are fetchable by the crawler.
        if !matches!(url.scheme(), "http" | "https") {
            return Err(crate::ScraperError::invalid_url(format!(
                "Scheme '{}' no soportado. Solo http:// y https://",
                url.scheme()
            )));
        }

        // Bug #675-5: strip credentials to prevent secret leaks
        // into logs, frontmatter, and exports.
        let _ = url.set_username("");
        let _ = url.set_password(None);

        Ok(Self(url))
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

/// A validated SHA-256 content digest in its lowercase-hex wire form (#1118).
///
/// The vector pipeline's dedup key used to be a raw `String`: any 64-char
/// non-hex string passed as a content hash and corrupted the key (false
/// collisions or misses). The only construction paths are
/// [`from_digest`](Self::from_digest) (infallible — from a real SHA-256
/// digest) and the string conversions below, which REJECT anything that is
/// not exactly 64 lowercase hex characters. Serde goes through
/// `TryFrom<String>`, so a malformed hash cannot enter or leave a
/// (de)serialized DTO either.
///
/// # Examples
///
/// ```
/// use webfang_core::domain::Sha256Hex;
///
/// let hex = "a".repeat(64);
/// let hash = Sha256Hex::try_from(hex.as_str()).expect("64 lowercase hex chars");
/// assert_eq!(hash.as_str(), hex);
/// assert!(Sha256Hex::try_from("deadbeef").is_err(), "short non-digest rejected");
/// assert!(Sha256Hex::try_from("z".repeat(64).as_str()).is_err(), "non-hex rejected");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Sha256Hex(String);

impl Sha256Hex {
    /// Wire length of a SHA-256 hex digest.
    pub const HEX_LEN: usize = 64;

    /// Wrap a real 32-byte SHA-256 digest (infallible — the bytes came
    /// from the hash function).
    #[must_use]
    pub fn from_digest(digest: [u8; 32]) -> Self {
        use std::fmt::Write as _;
        let mut hex = String::with_capacity(Self::HEX_LEN);
        for byte in digest {
            // A byte always renders as exactly two lowercase hex chars.
            let _ = write!(&mut hex, "{byte:02x}");
        }
        Self(hex)
    }

    /// The lowercase-hex wire form (64 chars).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reject anything that is not exactly [`Sha256Hex::HEX_LEN`] lowercase hex
/// characters. Uppercase is rejected on purpose: every producer in this
/// repo (`sha2::Sha256`) emits lowercase, and accepting both would let two
/// spellings of the same digest split the dedup key.
fn parse_sha256_hex(s: String) -> Result<Sha256Hex, crate::ScraperError> {
    let valid = s.len() == Sha256Hex::HEX_LEN
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if valid {
        Ok(Sha256Hex(s))
    } else {
        Err(crate::ScraperError::Validation(
            "sha256 hex inválido: se esperan exactamente 64 caracteres hexadecimales en minúscula"
                .to_string(),
        ))
    }
}

impl TryFrom<String> for Sha256Hex {
    type Error = crate::ScraperError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        parse_sha256_hex(value)
    }
}

impl TryFrom<&str> for Sha256Hex {
    type Error = crate::ScraperError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        parse_sha256_hex(value.to_string())
    }
}

impl std::str::FromStr for Sha256Hex {
    type Err = crate::ScraperError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl<'de> Deserialize<'de> for Sha256Hex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)
            .and_then(|s| parse_sha256_hex(s).map_err(<D::Error as serde::de::Error>::custom))
    }
}

impl std::fmt::Display for Sha256Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::LowerHex for Sha256Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
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
        assert_eq!(format!("{url}"), "https://example.com/");
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

    #[test]
    fn test_valid_url_rejects_invalid() {
        assert!(ValidUrl::parse("not-a-url").is_err());
    }

    #[test]
    fn test_valid_url_rejects_ftp_scheme() {
        let result = ValidUrl::parse("ftp://example.com/file");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("ftp"), "Error should mention the scheme");
    }

    #[test]
    fn test_valid_url_rejects_data_scheme() {
        assert!(ValidUrl::parse("data:text/html,<h1>hi</h1>").is_err());
        assert!(ValidUrl::parse("blob:https://example.com/uuid").is_err());
        assert!(ValidUrl::parse("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_valid_url_strips_credentials() {
        let url = ValidUrl::parse("https://user:pass@example.com/path")
            .expect("should parse valid https URL");
        assert_eq!(url.as_str(), "https://example.com/path");
        assert!(url.as_url().username().is_empty());
        assert!(url.as_url().password().is_none());
    }

    #[test]
    fn test_valid_url_accepts_http_https() {
        assert!(ValidUrl::parse("https://example.com").is_ok());
        assert!(ValidUrl::parse("http://example.com:8080/path?q=1").is_ok());
    }

    #[test]
    fn test_valid_url_no_credentials_unchanged() {
        let url = ValidUrl::parse("https://example.com/path").expect("plain https should parse");
        assert_eq!(url.as_str(), "https://example.com/path");
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
    fn test_correlation_id_child_shares_trace_id() {
        // Fase 1b (issue #356): a child correlation ID must keep the parent's
        // trace_id (so all pages of a crawl share one trace) while getting a
        // fresh span_id (so each page is a distinct span).
        let parent = CorrelationId::new();
        let child = parent.child();

        assert_eq!(
            parent.trace_id(),
            child.trace_id(),
            "child must share the parent's trace_id"
        );
        assert_ne!(
            parent.span_id(),
            child.span_id(),
            "child must get a fresh span_id"
        );
    }

    #[test]
    fn test_correlation_id_children_share_trace_id() {
        // Multiple children of the same parent all share the trace_id but have
        // distinct span_ids — this lets a whole crawl be reconstructed by
        // trace_id while keeping each page distinguishable.
        let parent = CorrelationId::new();
        let c1 = parent.child();
        let c2 = parent.child();

        assert_eq!(c1.trace_id(), c2.trace_id());
        assert_eq!(c1.trace_id(), parent.trace_id());
        assert_ne!(c1.span_id(), c2.span_id(), "siblings must differ");
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
        let display = format!("{corr}");

        assert!(display.starts_with("00-"));
        assert_eq!(display, corr.to_traceparent());
    }

    #[test]
    fn test_correlation_id_tracestate() {
        let corr = CorrelationId::new();
        let tracestate = corr.to_tracestate();

        // Format: webfang=v1:{32 hex trace_id}
        // webfang=v1: = 11 chars
        // trace_id = 32 chars
        // Total = 43 chars
        assert!(tracestate.starts_with("webfang=v1:"));
        assert!(tracestate.contains('='));
        assert_eq!(tracestate.len(), 43);
    }

    // -- Sha256Hex (#1118) ---------------------------------------------------

    #[test]
    fn sha256_hex_accepts_64_lowercase_hex() {
        let hex = "0123456789abcdef".repeat(4);
        assert_eq!(hex.len(), Sha256Hex::HEX_LEN);
        let hash = Sha256Hex::try_from(hex.as_str()).expect("valid digest hex");
        assert_eq!(hash.as_str(), hex);
    }

    /// #1118 reproduction: the dedup key used to be a raw `String`, so any
    /// 64-char non-hex value passed as a "hash". The newtype rejects it at
    /// every construction path, including serde.
    #[test]
    fn sha256_hex_rejects_non_hex_wrong_len_and_uppercase() {
        assert!(Sha256Hex::try_from("deadbeef").is_err(), "too short");
        assert!(
            Sha256Hex::try_from("z".repeat(64).as_str()).is_err(),
            "non-hex"
        );
        assert!(
            Sha256Hex::try_from("a".repeat(63).as_str()).is_err(),
            "63 chars"
        );
        assert!(
            Sha256Hex::try_from("A".repeat(64).as_str()).is_err(),
            "uppercase splits the key"
        );
        let json = format!("\"{}\"", "g".repeat(64));
        let err =
            serde_json::from_str::<Sha256Hex>(&json).expect_err("serde must reject non-hex too");
        assert!(err.to_string().contains("sha256"), "got: {err}");
    }

    #[test]
    fn sha256_hex_from_digest_roundtrips_wire_form() {
        let digest = [0xabu8; 32];
        let hash = Sha256Hex::from_digest(digest);
        assert_eq!(hash.as_str(), "ab".repeat(32));
        let back: Sha256Hex = hash.as_str().parse().expect("wire form re-parses");
        assert_eq!(back, hash);
    }

    // -- ValidUrl::TryFrom<url::Url> (#1117) ---------------------------------

    #[test]
    fn valid_url_try_from_url_applies_scheme_hardening() {
        // A joined `data:` URL (asset extractor hostile src) must be
        // rejected by the SAME gate as ValidUrl::parse.
        let base = url::Url::parse("https://example.com/").expect("base");
        let hostile = base.join("data:text/html,<h1>x</h1>").expect("joins");
        assert!(
            ValidUrl::try_from_url(hostile).is_err(),
            "data: must not pass"
        );
        let ok = base.join("/img/a.png").expect("joins");
        let valid = ValidUrl::try_from_url(ok).expect("https passes");
        assert_eq!(valid.as_str(), "https://example.com/img/a.png");
    }
}
