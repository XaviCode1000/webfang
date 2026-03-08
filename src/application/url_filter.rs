//! URL filtering module
//!
//! Provides URL pattern matching and filtering functionality for the crawler.
//!
//! # Rules Applied
//!
//! - **own-borrow-over-clone**: Accepts `&str` not `&String`, `&[T]` not `&Vec<T>`
//! - **opt-inline**: Inlines hot path functions
//! - **test-proptest-properties**: Property-based tests for pattern matching
//! - **url-no-string-split**: Uses url crate for RFC 3986 compliant parsing
//! - **security-ssrf-prevention**: Delegates to domain::matches_pattern (SSRF-safe host comparison)

use crate::domain::CrawlerConfig;

/// Check if a URL matches a glob-style pattern
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **opt-inline**: Inlined for hot path performance.
/// Following **security-ssrf-prevention**: Delegates to domain implementation (SSRF-safe).
///
/// # Security Note
///
/// This function uses SSRF-safe pattern matching that compares HOSTS only,
/// NOT full URL paths. Patterns like `*/admin/*` are NOT supported.
/// Use domain-based patterns like `*.example.com/*` instead.
///
/// # Arguments
///
/// * `url` - URL to test
/// * `pattern` - Glob-style pattern (* for wildcard)
///
/// # Returns
///
/// `true` if the URL matches the pattern
///
/// # Examples
///
/// ```
/// use rust_scraper::application::url_filter::matches_pattern;
///
/// assert!(matches_pattern("https://example.com/page", "*"));
/// assert!(matches_pattern("https://blog.example.com/post", "*.example.com/*"));
/// assert!(!matches_pattern("https://other.com/page", "example.com"));
/// ```
#[inline]
#[must_use]
pub fn matches_pattern(url: &str, pattern: &str) -> bool {
    // Delegate to domain implementation (SSRF-safe)
    crate::domain::matches_pattern(url, pattern)
}

/// Check if a URL is excluded by any of the exclude patterns
///
/// Following **own-borrow-over-clone**: Accepts `&[String]` not `&Vec<String>`.
/// Following **opt-inline**: Inlined for hot path performance.
///
/// # Arguments
///
/// * `url` - URL to test
/// * `patterns` - Slice of exclude patterns
///
/// # Returns
///
/// `true` if the URL matches any exclude pattern
///
/// # Examples
///
/// ```
/// use rust_scraper::application::url_filter::is_excluded;
///
/// let patterns = vec!["*.evil.com".to_string(), "*.malicious.com".to_string()];
/// assert!(is_excluded("https://evil.com/page", &patterns));
/// assert!(!is_excluded("https://example.com/page", &patterns));
/// ```
#[inline]
#[must_use]
pub fn is_excluded(url: &str, patterns: &[String]) -> bool {
    patterns.iter().any(|pattern| matches_pattern(url, pattern))
}

/// Check if a URL is allowed based on the crawler configuration
///
/// Following **own-borrow-over-clone**: Accepts `&CrawlerConfig` reference.
///
/// # Arguments
///
/// * `url` - URL to test
/// * `config` - Crawler configuration with include/exclude patterns
///
/// # Returns
///
/// `true` if the URL is allowed (matches include and doesn't match exclude)
///
/// # Examples
///
/// ```
/// use rust_scraper::{application::url_filter::is_allowed, domain::CrawlerConfig};
/// use url::Url;
///
/// let seed = Url::parse("https://example.com").unwrap();
/// let config = CrawlerConfig::builder(seed)
///     .include_pattern("*.example.com/*".to_string())
///     .exclude_pattern("*.evil.com".to_string())
///     .build();
///
/// assert!(is_allowed("https://example.com/page", &config));
/// assert!(!is_allowed("https://evil.com/page", &config));
/// assert!(!is_allowed("https://other.com/page", &config));
/// ```
#[inline]
#[must_use]
pub fn is_allowed(url: &str, config: &CrawlerConfig) -> bool {
    // First check exclude patterns (deny takes precedence)
    if is_excluded(url, &config.exclude_patterns) {
        return false;
    }

    // Then check include patterns (if any are specified)
    config.matches_include(url)
}

/// Extract domain from URL using url crate (RFC 3986 compliant)
///
/// Following **url-no-string-split**: Uses `url::Url::parse().host_str()`
/// instead of string splitting for proper RFC 3986 compliance.
///
/// Handles:
/// - Credentials: `http://user:pass@domain.com` → `domain.com`
/// - Ports: `https://domain.com:8080/path` → `domain.com`
/// - IPv6: `http://[::1]:8080` → `[::1]`
///
/// # Arguments
///
/// * `url` - URL to extract domain from
///
/// # Returns
///
/// Domain string or None if URL is invalid
///
/// # Examples
///
/// ```
/// use rust_scraper::application::url_filter::extract_domain;
///
/// assert_eq!(extract_domain("https://example.com/page"), Some("example.com".to_string()));
/// assert_eq!(extract_domain("https://blog.example.com/post"), Some("blog.example.com".to_string()));
/// assert_eq!(extract_domain("http://user:pass@domain.com:8080/path"), Some("domain.com".to_string()));
/// assert_eq!(extract_domain("http://[::1]:8080"), Some("[::1]".to_string()));
/// ```
#[inline]
#[must_use]
pub fn extract_domain(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
}

/// Check if a URL is internal (same domain as seed)
///
/// Following **own-borrow-over-clone**: Accepts `&str` for both parameters.
///
/// # Arguments
///
/// * `url` - URL to check
/// * `seed_domain` - Domain of the seed URL
///
/// # Returns
///
/// `true` if the URL belongs to the same domain
///
/// # Examples
///
/// ```
/// use rust_scraper::application::url_filter::is_internal_link;
///
/// assert!(is_internal_link("https://example.com/page", "example.com"));
/// assert!(is_internal_link("https://www.example.com/page", "example.com"));
/// assert!(!is_internal_link("https://other.com/page", "example.com"));
/// ```
#[inline]
#[must_use]
pub fn is_internal_link(url: &str, seed_domain: &str) -> bool {
    extract_domain(url)
        .map(|domain| domain == seed_domain || domain.ends_with(&format!(".{}", seed_domain)))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_matches_pattern_wildcard() {
        assert!(matches_pattern("https://example.com/page", "*"));
        assert!(matches_pattern("https://any.domain/any/path", "*"));
    }

    #[test]
    fn test_matches_pattern_domain_wildcard() {
        assert!(matches_pattern(
            "https://blog.example.com/post",
            "*.example.com/*"
        ));
        assert!(matches_pattern(
            "https://sub.example.com/page",
            "*.example.com"
        ));
        assert!(!matches_pattern("https://other.com/page", "*.example.com"));
    }

    #[test]
    fn test_matches_pattern_exact_host() {
        // Exact host match (no path matching in SSRF-safe version)
        // Pattern matches HOST only, path is ignored
        assert!(matches_pattern(
            "https://example.com/page",
            "example.com"
        ));
        // Same host, different path - STILL MATCHES (host-only comparison)
        assert!(matches_pattern(
            "https://example.com/other",
            "example.com"
        ));
        // Different host - no match
        assert!(!matches_pattern(
            "https://other.com/page",
            "example.com"
        ));
    }

    #[test]
    fn test_matches_pattern_empty() {
        assert!(matches_pattern("https://example.com", ""));
    }

    #[test]
    fn test_is_excluded() {
        // SSRF-safe: patterns match HOSTS only
        let patterns = vec![
            "*.evil.com".to_string(),
            "*.malicious.com".to_string(),
            "*.example.com".to_string(), // Exclude all example.com subdomains
        ];

        assert!(is_excluded("https://evil.com/page", &patterns));
        assert!(is_excluded("https://blog.evil.com/admin", &patterns));
        assert!(is_excluded("https://malicious.com/data", &patterns));
        assert!(is_excluded("https://blog.example.com/login", &patterns));
        // example.com matches *.example.com pattern (host-only matching)
        assert!(is_excluded("https://example.com/public/page", &patterns));
        assert!(!is_excluded("https://good.com/page", &patterns));
    }

    #[test]
    fn test_is_allowed() {
        let seed = Url::parse("https://example.com").unwrap();

        // Config with include and exclude patterns (HOSTS only)
        let config = CrawlerConfig::builder(seed)
            .include_pattern("*.example.com/*".to_string())
            .exclude_pattern("*.evil.com".to_string())
            .build();

        // Allowed: matches include, doesn't match exclude
        assert!(is_allowed("https://example.com/page", &config));
        assert!(is_allowed("https://blog.example.com/post", &config));

        // Denied: matches exclude
        assert!(!is_allowed("https://evil.com/page", &config));
        assert!(!is_allowed("https://blog.evil.com/admin", &config));

        // Denied: doesn't match include
        assert!(!is_allowed("https://other.com/page", &config));
    }

    #[test]
    fn test_is_allowed_no_patterns() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::new(seed);

        // No patterns = everything allowed
        assert!(is_allowed("https://example.com/page", &config));
        assert!(is_allowed("https://other.com/page", &config));
    }

    #[test]
    fn test_is_allowed_include_only() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .include_pattern("*.example.com/*".to_string())
            .build();

        // Matches include pattern
        assert!(is_allowed("https://example.com/page", &config));
        assert!(is_allowed("https://blog.example.com/post", &config));

        // Doesn't match include pattern
        assert!(!is_allowed("https://other.com/page", &config));
    }

    #[test]
    fn test_is_allowed_exclude_only() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::builder(seed)
            .exclude_pattern("*.evil.com".to_string())
            .build();

        // Doesn't match exclude pattern
        assert!(is_allowed("https://example.com/page", &config));
        assert!(is_allowed("https://example.com/public/page", &config));

        // Matches exclude pattern
        assert!(!is_allowed("https://evil.com/page", &config));
        assert!(!is_allowed("https://blog.evil.com/admin", &config));
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(
            extract_domain("https://example.com/page"),
            Some("example.com".to_string())
        );
        assert_eq!(
            extract_domain("https://blog.example.com/post"),
            Some("blog.example.com".to_string())
        );
        assert_eq!(
            extract_domain("http://sub.domain.example.com/path"),
            Some("sub.domain.example.com".to_string())
        );
        assert_eq!(extract_domain("invalid-url"), None);
    }

    #[test]
    fn test_extract_domain_with_credentials() {
        assert_eq!(
            extract_domain("http://user:pass@domain.com/path"),
            Some("domain.com".to_string())
        );
    }

    #[test]
    fn test_extract_domain_with_port() {
        assert_eq!(
            extract_domain("https://domain.com:8080/path"),
            Some("domain.com".to_string())
        );
    }

    #[test]
    fn test_extract_domain_ipv6() {
        assert_eq!(
            extract_domain("http://[::1]:8080"),
            Some("[::1]".to_string())
        );
    }

    #[test]
    fn test_is_internal_link() {
        assert!(is_internal_link("https://example.com/page", "example.com"));
        assert!(is_internal_link(
            "https://www.example.com/page",
            "example.com"
        ));
        assert!(is_internal_link(
            "https://blog.example.com/post",
            "example.com"
        ));
        assert!(!is_internal_link("https://other.com/page", "example.com"));
        assert!(!is_internal_link("invalid-url", "example.com"));
    }

    #[test]
    fn test_is_allowed_complex_scenarios() {
        let seed = Url::parse("https://example.com").unwrap();

        // Multiple include patterns (HOSTS only)
        let config = CrawlerConfig::builder(seed)
            .include_pattern("*.blog.example.com/*".to_string())
            .include_pattern("*.docs.example.com/*".to_string())
            .exclude_pattern("*.draft.example.com".to_string())
            .build();

        // Allowed: matches blog include
        assert!(is_allowed("https://blog.example.com/post-1", &config));
        assert!(is_allowed("https://my.blog.example.com/post-1", &config));

        // Allowed: matches docs include
        assert!(is_allowed("https://docs.example.com/guide", &config));

        // Denied: matches exclude
        assert!(!is_allowed("https://draft.example.com/post", &config));
        assert!(!is_allowed("https://my.draft.example.com/guide", &config));

        // Denied: doesn't match any include
        assert!(!is_allowed("https://example.com/shop/products", &config));
        assert!(!is_allowed("https://other.com/page", &config));
    }

    // ========== SSRF PREVENTION TESTS ==========

    #[test]
    fn test_matches_pattern_ssrf_bypass_attempt() {
        // Evil URL with query params containing target domain should NOT match
        assert!(!matches_pattern(
            "https://evil.com/?q=example.com/path",
            "*.example.com/*"
        ));

        assert!(!matches_pattern(
            "https://attacker.com/?redirect=example.com/admin",
            "*.example.com/*"
        ));

        assert!(!matches_pattern(
            "https://malicious.com/redirect?url=example.com/secret",
            "*.example.com/*"
        ));
    }

    #[test]
    fn test_is_excluded_ssrf_safe() {
        // SSRF bypass attempt should NOT be excluded (different host)
        let patterns = vec!["*.example.com".to_string()];
        
        // Evil domain should NOT match example.com pattern
        assert!(!is_excluded("https://evil.com/?q=example.com/admin", &patterns));
        
        // Real example.com subdomain SHOULD match
        assert!(is_excluded("https://admin.example.com/page", &patterns));
    }
}
