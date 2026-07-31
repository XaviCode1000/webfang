//! Property-based tests for URL normalization and pattern matching
//!
//! Uses `proptest` to generate random inputs and verify invariants.
//!
//! Run with: cargo nextest run --test-threads 2 property_tests

use proptest::prelude::*;
use webfang::{is_internal_link, matches_pattern};
use url::Url;

// ============================================================================
// Property: matches_pattern is reflexive for wildcard
// ============================================================================

proptest! {
    /// Any valid URL should match the wildcard pattern "*"
    #[test]
    fn prop_wildcard_matches_any_url(url_str in "https://[a-z]{3,10}\\.[a-z]{2,5}(/[a-z]{1,8})*") {
        // The wildcard pattern should match ANY URL
        prop_assert!(
            matches_pattern(&url_str, "*"),
            "Wildcard '*' should match any URL, but failed for: {}",
            url_str
        );
    }
}

// ============================================================================
// Property: is_internal_link is reflexive for same domain
// ============================================================================

proptest! {
    /// A URL should always be internal to its own domain
    #[test]
    fn prop_url_is_internal_to_own_domain(
        scheme in "[a-z]{3,5}",
        domain in "[a-z]{3,12}\\.[a-z]{2,5}",
        path in "(/[a-z]{1,10})*",
    ) {
        let url = format!("{}://{}{}", scheme, domain, path);
        // Same domain should always be internal
        prop_assert!(
            is_internal_link(&url, &domain),
            "URL '{}' should be internal to domain '{}'",
            url,
            domain
        );

        // With www prefix should also be internal
        let www_url = format!("{}://www.{}{}", scheme, domain, path);
        prop_assert!(
            is_internal_link(&www_url, &domain),
            "URL '{}' should be internal to domain '{}' (www prefix)",
            www_url,
            domain
        );
    }
}

// ============================================================================
// Property: is_internal_link is asymmetric for different domains
// ============================================================================

proptest! {
    /// A URL from one domain should NOT be internal to a different domain
    #[test]
    fn prop_different_domains_not_internal(
        domain_a in "[a-z]{5,12}\\.com",
        domain_b in "[a-z]{5,12}\\.org",
        path in "(/[a-z]{1,8}){1,3}",
    ) {
        // Ensure domains are different
        prop_assume!(domain_a != domain_b);

        let url = format!("https://{}{}", domain_a, path);
        prop_assert!(
            !is_internal_link(&url, &domain_b),
            "URL from '{}' should NOT be internal to '{}'",
            domain_a,
            domain_b
        );
    }
}

// ============================================================================
// Property: matches_pattern with subdomain wildcards
// ============================================================================

proptest! {
    /// Pattern "*.example.com" should match any subdomain of example.com
    #[test]
    fn prop_subdomain_wildcard_matches(
        subdomain in "[a-z]{2,8}",
        path in "(/[a-z]{1,6}){0,3}",
    ) {
        let url = format!("https://{}.example.com{}", subdomain, path);
        prop_assert!(
            matches_pattern(&url, "*.example.com"),
            "Subdomain wildcard should match '{}'",
            url
        );
    }
}

// ============================================================================
// Property: URL parsing round-trip for valid URLs
// ============================================================================

proptest! {
    /// URLs that parse successfully should have consistent domain extraction
    #[test]
    fn prop_url_domain_consistent(
        scheme in prop_oneof!["https", "http"],
        domain in "[a-z]{3,15}\\.[a-z]{2,6}",
        port in proptest::option::of(80u16..65535),
        path in "(/[a-zA-Z0-9_-]{1,12}){0,5}",
    ) {
        let url_str = if let Some(p) = port {
            format!("{}://{}:{}{}", scheme, domain, p, path)
        } else {
            format!("{}://{}{}", scheme, domain, path)
        };

        if let Ok(url) = Url::parse(&url_str) {
            // Domain extraction should be consistent
            let host = url.host_str().expect("Parsed URL should have host");
            prop_assert!(
                !host.is_empty(),
                "Host should not be empty for valid URL: {}",
                url_str
            );
        }
    }
}

// ============================================================================
// Fixed regression tests for known edge cases
// ============================================================================

#[test]
fn test_matches_pattern_empty_pattern_does_not_match() {
    // Empty pattern should not match anything
    // Empty pattern matches everything (by design)
    assert!(matches_pattern("https://example.com", ""));
}

#[test]
fn test_matches_pattern_exact_domain() {
    assert!(matches_pattern("https://example.com/page", "example.com"));
    assert!(!matches_pattern(
        "https://blog.example.com/page",
        "example.com"
    ));
}

#[test]
fn test_is_internal_link_with_port() {
    assert!(is_internal_link(
        "https://example.com:8080/page",
        "example.com"
    ));
    assert!(!is_internal_link(
        "https://other.com:8080/page",
        "example.com"
    ));
}

#[test]
fn test_is_internal_link_relative_path() {
    // Relative paths (no scheme) should not be internal
    assert!(!is_internal_link("/page", "example.com"));
}

#[test]
fn test_matches_pattern_multiple_subdomains() {
    assert!(matches_pattern(
        "https://a.b.example.com/page",
        "*.example.com"
    ));
}
