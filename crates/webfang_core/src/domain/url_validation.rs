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

/// Normalize a seed reference to a bare host string (no scheme, port, or path).
///
/// Accepts both full URLs and bare hosts:
/// - Full URL: `https://example.com:9090/path` → `example.com` (via
///   [`url::Url::host_str`], which drops the port).
/// - Bare host: `example.com/path` → `example.com` (path stripped) and
///   `example.com:8080` → `example.com` (port stripped).
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
/// The bare host component.
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
/// ```
#[inline]
#[must_use]
pub fn normalize_seed_host(seed: &str) -> String {
    if let Ok(u) = url::Url::parse(seed) {
        if let Some(host) = u.host_str() {
            return host.to_string();
        }
    }
    // Bare host: strip any accidental path suffix, then any :port suffix.
    let without_path = seed.split('/').next().unwrap_or(seed);
    without_path
        .split(':')
        .next()
        .unwrap_or(without_path)
        .to_string()
}

/// Check whether a URL is internal to a seed domain (same host or subdomain).
///
/// Both the URL host and the seed are normalized to bare hosts (port stripped)
/// before comparison, so URLs with an explicit port are handled correctly. This
/// is the single canonical implementation shared by the crawl engine and the MCP
/// `is_internal_link` tool (#479).
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
/// ```
#[inline]
#[must_use]
pub fn is_internal_link(url: &str, seed_domain: &str) -> bool {
    let seed_host = normalize_seed_host(seed_domain);
    extract_domain(url)
        .map(|host| host == seed_host || host.ends_with(&format!(".{seed_host}")))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
