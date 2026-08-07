//! URL validation utilities.

use crate::error::ScraperError;
use crate::Result;

/// Validate and parse a URL string using the `url` crate (RFC 3986 compliant).
///
/// This function performs strict URL validation:
/// - Trims whitespace automatically
/// - Requires http or https scheme (case-insensitive)
/// - Requires a valid host
/// - Rejects malformed URLs
///
/// # Arguments
///
/// * `url` - URL string to validate and parse
///
/// # Returns
///
/// * `Ok(url::Url)` - Validated and parsed URL
/// * `Err(ScraperError::InvalidUrl)` - Invalid URL with error message
///
/// # Errors
///
/// Returns an error if:
/// - URL is empty
/// - URL has invalid format
/// - URL scheme is not http or https
/// - URL has no host
///
/// # Examples
///
/// ```
/// use webfang_core::validate_and_parse_url;
///
/// // Valid URLs
/// let url = validate_and_parse_url("https://example.com").unwrap();
/// assert_eq!(url.host_str(), Some("example.com"));
///
/// let url = validate_and_parse_url("HTTP://EXAMPLE.COM").unwrap();
/// assert_eq!(url.scheme(), "http");
///
/// // Invalid URLs
/// assert!(validate_and_parse_url("").is_err());
/// assert!(validate_and_parse_url("ftp://example.com").is_err());
/// assert!(validate_and_parse_url("not-a-url").is_err());
/// ```
///
/// # Whitespace Handling
///
/// Leading and trailing whitespace is automatically trimmed:
///
/// ```
/// use webfang_core::validate_and_parse_url;
///
/// let url = validate_and_parse_url("  https://example.com  ").unwrap();
/// assert_eq!(url.host_str(), Some("example.com"));
/// ```
pub fn validate_and_parse_url(url: &str) -> Result<url::Url> {
    if url.is_empty() {
        return Err(ScraperError::invalid_url("URL cannot be empty"));
    }

    let parsed = url::Url::parse(url.trim())
        .map_err(|e| ScraperError::invalid_url(format!("Failed to parse URL '{url}': {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {},
        scheme => {
            return Err(ScraperError::invalid_url(format!(
                "URL must use http or https scheme, got '{scheme}'"
            )))
        },
    }

    if parsed.host_str().is_none() {
        return Err(ScraperError::invalid_url("URL must have a valid host"));
    }

    Ok(parsed)
}

/// Extract the host (domain) from a URL, without any port component.
///
/// Uses [`url::Url::host_str`] for RFC 3986 compliant parsing. Unlike naive
/// `split(':')` approaches, this correctly returns the host WITHOUT the port:
/// - Ports: `http://127.0.0.1:8080/page` → `127.0.0.1`
/// - Credentials: `http://user:pass@domain.com` → `domain.com`
/// - IPv6: `http://[::1]:8080` → `[::1]`
///
/// # Arguments
///
/// * `url` - URL to extract the host from
///
/// # Returns
///
/// The host without port, or `None` if the URL cannot be parsed or has no host.
///
/// # Examples
///
/// ```
/// use webfang_core::domain::url_validation::extract_domain;
///
/// assert_eq!(extract_domain("http://127.0.0.1:8080/page"), Some("127.0.0.1".to_string()));
/// assert_eq!(extract_domain("https://example.com/path"), Some("example.com".to_string()));
/// assert_eq!(extract_domain("http://[::1]:8080"), Some("[::1]".to_string()));
/// assert_eq!(extract_domain("not-a-url"), None);
/// ```
#[inline]
#[must_use]
pub fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
}

/// Normalize a seed reference to a bare host string (no scheme, port, path, or
/// `www.` prefix).
///
/// Accepts both full URLs and bare hosts:
/// - Full URL: `https://example.com:9090/path` → `example.com` (via
///   [`url::Url::host_str`], which drops the port).
/// - Bare host: `example.com/path` → `example.com` (path stripped) and
///   `example.com:8080` → `example.com` (port stripped).
///
/// The `www.` prefix is stripped so that seed hosts are compared on the same
/// footing as links normalized with `strip_www=true` (#500). Without this, a
/// `www.`-prefixed seed (`www.gnu.org`) never matches its stripped links
/// (`gnu.org`), silently dropping every internal URL on most real-world sites.
///
/// This is the canonical seed-host normalizer shared by the crawl engine and the
/// MCP `is_internal_link` tool, so both classify links against the same bare host.
///
/// # Arguments
///
/// * `seed` - Seed URL or bare host to normalize
///
/// # Returns
///
/// The bare host component, without a leading `www.`.
///
/// # Examples
///
/// ```
/// use webfang_core::domain::url_validation::normalize_seed_host;
///
/// assert_eq!(normalize_seed_host("https://example.com/path"), "example.com");
/// assert_eq!(normalize_seed_host("example.com"), "example.com");
/// assert_eq!(normalize_seed_host("example.com/path"), "example.com");
/// assert_eq!(normalize_seed_host("example.com:8080"), "example.com");
/// // www stripping (#500):
/// assert_eq!(normalize_seed_host("https://www.example.com/path"), "example.com");
/// assert_eq!(normalize_seed_host("www.example.com"), "example.com");
/// ```
#[inline]
#[must_use]
pub fn normalize_seed_host(seed: &str) -> String {
    // Try full-URL parse first; fall through to bare-host handling when the
    // input has no authority component (e.g. "www.example.com:8080" parses as
    // scheme "www.example.com" with no host).
    if let Ok(u) = url::Url::parse(seed) {
        if let Some(h) = u.host_str() {
            return strip_www_prefix(h);
        }
    }
    // Bare host: strip any accidental path suffix, then any :port suffix.
    let without_path = seed.split('/').next().unwrap_or(seed);
    let host = without_path.split(':').next().unwrap_or(without_path);
    strip_www_prefix(host)
}

/// Strip a leading `www.` from a host string, returning the bare domain.
///
/// Consistent with `normalize_url(strip_www=true)` used on the link side,
/// so both sides of the internal-link comparison are www-agnostic (#500).
#[inline]
fn strip_www_prefix(host: &str) -> String {
    host.strip_prefix("www.")
        .map(String::from)
        .unwrap_or_else(|| host.to_string())
}

/// Check whether a URL is internal to a seed domain (same host or subdomain).
///
/// Both the URL host and the seed are normalized to bare hosts (port and
/// `www.` stripped) before comparison, so URLs with an explicit port or a
/// `www.` prefix mismatch are handled correctly. This is the single canonical
/// implementation shared by the crawl engine and the MCP `is_internal_link`
/// tool (#479, #500).
///
/// # Arguments
///
/// * `url` - URL to classify
/// * `seed_domain` - Seed host or full URL to compare against
///
/// # Returns
///
/// `true` if `url`'s host equals the seed host or is a subdomain of it.
///
/// # Examples
///
/// ```
/// use webfang_core::domain::url_validation::is_internal_link;
///
/// assert!(is_internal_link("https://example.com/page", "example.com"));
/// assert!(is_internal_link("https://blog.example.com/post", "example.com"));
/// // The port case that broke the crawl engine (#479):
/// assert!(is_internal_link("http://127.0.0.1:8080/page", "127.0.0.1"));
/// assert!(!is_internal_link("http://other.com:8080/x", "example.com"));
/// // The www asymmetry that broke DOM discovery (#500):
/// assert!(is_internal_link("https://gnu.org/page", "www.gnu.org"));
/// assert!(is_internal_link("https://www.gnu.org/page", "gnu.org"));
/// assert!(is_internal_link("https://www.gnu.org/page", "www.gnu.org"));
/// ```
#[inline]
#[must_use]
pub fn is_internal_link(url: &str, seed_domain: &str) -> bool {
    let seed_host = normalize_seed_host(seed_domain);
    extract_domain(url)
        .map(|host| host == seed_host || host.ends_with(&format!(".{seed_host}")))
        .unwrap_or(false)
}

/// Normalize a URL (remove fragments, strip www, remove default ports, etc.)
///
/// This is the **canonical** URL normalizer for the scraper. All URL
/// normalization should go through this function. It lives in the domain layer
/// so both the application deduplicators and the infrastructure crawlers share
/// one canonical form for URL equivalence — two URLs that normalize to the same
/// string are treated as the same document (#517).
///
/// Options:
/// - `strip_hash: true` — removes URL fragments (`#section`)
/// - `strip_www` — removes `www.` prefix when `true` (caller controls behavior)
/// - `remove_trailing_slash: false` — preserves trailing slashes on nested
///   paths (`/page/` stays distinct from `/page`); a bare root is still
///   canonicalized to `https://host` by url-normalize
/// - `remove_query_parameters: All` — strips query strings for dedup
/// - `sort_query_parameters: true` — consistent ordering
///
/// After normalization, a trailing `/index.html` or `/index.htm` path segment
/// is collapsed to `/` (case-insensitive, idempotent) so the same document is
/// not stored under two URLs.
///
/// Non-URLs (strings without a `://` scheme) are returned unchanged — a bare
/// `not-a-valid-url` is never coerced into `http://not-a-valid-url`.
///
/// # Arguments
///
/// * `url` - URL to normalize
/// * `strip_www` - If `true`, removes `www.` prefix (e.g. `www.example.com` → `example.com`)
///
/// # Examples
///
/// ```
/// use webfang_core::domain::url_validation::normalize_url;
///
/// assert_eq!(
///     normalize_url("https://example.com/page#section", true),
///     "https://example.com/page"
/// );
/// assert_eq!(
///     normalize_url("https://www.example.com/page", true),
///     "https://example.com/page"
/// );
/// assert_eq!(
///     normalize_url("https://www.example.com/page", false),
///     "https://www.example.com/page"
/// );
/// assert_eq!(
///     normalize_url("https://example.com:443/page", true),
///     "https://example.com/page"
/// );
/// assert_eq!(
///     normalize_url("https://example.com/index.html", true),
///     "https://example.com"
/// );
/// ```
#[inline]
#[must_use]
pub fn normalize_url(url: &str, strip_www: bool) -> String {
    use url_normalize::{normalize_url as normalize, Options, RemoveQueryParameters};

    // Non-URLs (no scheme) should not be normalized — return as-is.
    // This prevents "not-a-valid-url" → "http://not-a-valid-url" conversion.
    if !url.contains("://") {
        return url.to_string();
    }

    let opts = Options {
        strip_hash: true,
        remove_trailing_slash: false,
        remove_query_parameters: RemoveQueryParameters::All,
        sort_query_parameters: true,
        strip_www,
        force_https: false,
        ..Options::default()
    };

    // url-normalize handles WHATWG preprocessing (control chars, backslashes,
    // trailing whitespace) and produces idempotent output.
    let normalized = normalize(url, &opts).unwrap_or_else(|_| url.to_string());

    // Collapse /index.html and /index.htm to / so the same document is not
    // stored under two URLs (idempotent, case-insensitive).
    collapse_index_path(&normalized)
}

/// Collapse a trailing `/index.html` or `/index.htm` path segment to `/`.
///
/// Many servers serve identical content at both `/` and `/index.html`;
/// collapsing them lets the crawler deduplicate what would otherwise be two
/// URLs pointing at the same document.
///
/// Guarantees:
/// - Case-insensitive filename match (`/Index.HTML` collapses too).
/// - Idempotent — an already-collapsed `/` path is returned unchanged.
/// - Anchored on a `/` segment boundary, so `/my-index.html` is NOT collapsed.
/// - Only `index.html` / `index.htm` — never `/index.php`, `/default.aspx`, etc.
///
/// If the normalized URL cannot be re-parsed, it is returned unchanged.
#[inline]
#[must_use]
fn collapse_index_path(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };

    let lower = parsed.path().to_ascii_lowercase();
    let collapsed = lower
        .strip_suffix("/index.html")
        .or_else(|| lower.strip_suffix("/index.htm"))
        .map(|parent| format!("{parent}/"));

    match collapsed {
        Some(new_path) => {
            parsed.set_path(&new_path);
            let mut result = parsed.to_string();
            // The `url` crate serializes a root path as "https://host/", but
            // url-normalize canonicalizes a bare root to "https://host" (no
            // slash). Mirror that so "/" and "/index.html" produce the SAME
            // string and deduplicate (#344). Nested paths keep their slash.
            if parsed.path() == "/" {
                result.pop();
            }
            result
        },
        None => url.to_string(),
    }
}

/// Return a URL path in canonical form for prefix/dedup comparison.
///
/// Strips a single trailing slash (except for the root `/`) so that `/docs/`
/// and `/docs` compare equal. This is the canonical form used by
/// sitemap-relevance filtering — anywhere two paths that differ only by a
/// trailing slash must be treated as the same section.
///
/// # Examples
///
/// ```
/// use webfang_core::domain::url_validation::canonical_path;
///
/// assert_eq!(canonical_path("/docs/"), "/docs");
/// assert_eq!(canonical_path("/docs"), "/docs");
/// assert_eq!(canonical_path("/"), "/");
/// assert_eq!(canonical_path(""), "");
/// ```
pub fn canonical_path(path: &str) -> &str {
    if path.len() > 1 && path.ends_with('/') {
        &path[..path.len() - 1]
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical_path_strips_trailing_slash() {
        assert_eq!(canonical_path("/docs/"), "/docs");
        assert_eq!(canonical_path("/docs"), "/docs");
        assert_eq!(canonical_path("/"), "/");
        assert_eq!(canonical_path(""), "");
        assert_eq!(canonical_path("/a/b/c/"), "/a/b/c");
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

    #[test]
    fn test_validate_and_parse_url_whitespace() {
        let url = validate_and_parse_url("  https://example.com  ");
        assert!(url.is_ok());
        assert_eq!(url.unwrap().host_str(), Some("example.com"));
    }

    // ========================================================================
    // Port-safety regression tests (#479)
    //
    // The crawl engine seeded `seed_domain` from `Url::host_str()` (port
    // stripped) but compared it against a host extracted with a naive
    // `split('/')` that KEPT the port. Any URL with an explicit port (wiremock,
    // non-standard ports) silently filtered every internal link. These tests
    // pin the corrected, port-safe behavior of the canonical component.
    // ========================================================================

    #[test]
    fn extract_domain_strips_port() {
        assert_eq!(
            extract_domain("http://127.0.0.1:8080/page"),
            Some("127.0.0.1".to_string())
        );
    }

    #[test]
    fn is_internal_link_loopback_with_port_is_internal() {
        // THE bug case: seed from host_str() has no port, URL keeps its port.
        assert!(is_internal_link("http://127.0.0.1:8080/page", "127.0.0.1"));
    }

    #[test]
    fn is_internal_link_localhost_with_port_is_internal() {
        assert!(is_internal_link("http://localhost:3000/a", "localhost"));
    }

    #[test]
    fn is_internal_link_subdomain_with_port_is_internal() {
        assert!(is_internal_link(
            "http://blog.example.com:8080/post",
            "example.com"
        ));
    }

    #[test]
    fn is_internal_link_seed_as_full_url_different_port_is_internal() {
        assert!(is_internal_link(
            "http://example.com:8080/x",
            "http://example.com:9090"
        ));
    }

    #[test]
    fn is_internal_link_other_host_with_port_is_external() {
        assert!(!is_internal_link("http://other.com:8080/x", "example.com"));
    }

    #[test]
    fn normalize_seed_host_handles_url_port_path_and_bare() {
        assert_eq!(
            normalize_seed_host("https://example.com/path"),
            "example.com"
        );
        assert_eq!(
            normalize_seed_host("http://example.com:9090"),
            "example.com"
        );
        assert_eq!(normalize_seed_host("example.com"), "example.com");
        assert_eq!(normalize_seed_host("example.com/path"), "example.com");
        assert_eq!(normalize_seed_host("example.com:8080"), "example.com");
        assert_eq!(normalize_seed_host(""), "");
    }

    // ========================================================================
    // www-agnostic regression tests (#500)
    //
    // DOM-mode crawl normalized links with `normalize_url(strip_www=true)`
    // but passed the seed host from `Url::host_str()` WITHOUT stripping www.
    // On www-prefixed sites (the majority of the web), every internal link
    // was classified external → 0 discovered URLs. These tests pin the
    // www-agnostic behavior of the canonical component in all four
    // seed/link www combinations.
    // ========================================================================

    #[test]
    fn normalize_seed_host_strips_www_from_url() {
        assert_eq!(normalize_seed_host("https://www.gnu.org/"), "gnu.org");
    }

    #[test]
    fn normalize_seed_host_strips_www_from_bare_host() {
        assert_eq!(normalize_seed_host("www.gnu.org"), "gnu.org");
    }

    #[test]
    fn normalize_seed_host_strips_www_with_port() {
        assert_eq!(normalize_seed_host("www.example.com:8080"), "example.com");
    }

    #[test]
    fn normalize_seed_host_preserves_non_www_subdomain() {
        // "wiki.example.com" must NOT be stripped — only leading "www." is.
        assert_eq!(normalize_seed_host("wiki.example.com"), "wiki.example.com");
    }

    #[test]
    fn is_internal_link_seed_www_link_stripped_is_internal() {
        // THE #500 bug case: seed has www, link was normalized with strip_www.
        assert!(is_internal_link("https://gnu.org/page", "www.gnu.org"));
    }

    #[test]
    fn is_internal_link_seed_stripped_link_www_is_internal() {
        // Reverse: seed bare, link keeps www (already worked via ends_with).
        assert!(is_internal_link("https://www.gnu.org/page", "gnu.org"));
    }

    #[test]
    fn is_internal_link_both_www_is_internal() {
        assert!(is_internal_link(
            "https://www.example.com/page",
            "www.example.com"
        ));
    }

    #[test]
    fn is_internal_link_neither_www_is_internal() {
        assert!(is_internal_link("https://example.com/page", "example.com"));
    }

    #[test]
    fn is_internal_link_www_seed_subdomain_link_is_internal() {
        // www seed + subdomain link: blog.example.com is internal to www.example.com.
        assert!(is_internal_link(
            "https://blog.example.com/post",
            "www.example.com"
        ));
    }

    #[test]
    fn is_internal_link_www_seed_other_host_is_external() {
        assert!(!is_internal_link("https://other.com/x", "www.example.com"));
    }

    /// Bug #5 regression: normalize_url handles scheme-first order correctly.
    /// Non-URLs (no ://) are returned as-is; Unicode URLs are normalized
    /// without double-encoding (issue #590).
    #[test]
    fn normalize_url_non_url_returned_as_is() {
        let result = normalize_url("not-a-url", false);
        assert_eq!(result, "not-a-url", "non-URL must be returned unchanged");
    }

    #[test]
    fn normalize_url_unicode_preserved_or_encoded() {
        // A Unicode URL should be handled without panicking. The exact
        // encoding depends on the url-normalize crate, but the result must
        // be a valid non-empty string (issue #590, bug #5).
        let input = "https://example.com/página";
        let result = normalize_url(input, false);
        assert!(!result.is_empty(), "result must not be empty");
        assert!(
            result.starts_with("https://example.com"),
            "scheme and host must be preserved, got: {result}"
        );
    }

    #[test]
    fn is_internal_link_www_seed_full_url_is_internal() {
        // Seed passed as full URL with www (MCP tool path).
        assert!(is_internal_link(
            "https://gnu.org/page",
            "https://www.gnu.org/"
        ));
    }
}
