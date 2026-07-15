//! Crawler integration tests
//!
//! Integration tests for the web crawler functionality.
//! These tests require network access and test against real websites.
//!
//! Run with: `cargo test --test crawler_integration -- --ignored`
//! (Tests are ignored by default to avoid network calls in CI)

use webfang::{
    crawl_site, discover_urls_for_tui, is_allowed, is_excluded, is_internal_link, matches_pattern,
    CrawlerConfig,
};
use url::Url;

/// Test pattern matching functionality
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
fn test_matches_pattern_prefix_wildcard() {
    // Host-only patterns (no leading '/') match HOSTS only
    assert!(matches_pattern(
        "https://blog.example.com/post",
        "*.example.com/*"
    ));
    assert!(matches_pattern(
        "https://admin.example.com/users",
        "*.example.com/*"
    ));
    // Different domain should not match
    assert!(!matches_pattern(
        "https://other.com/page",
        "*.example.com/*"
    ));
    // Root domain does NOT match *.example.com/* (must be subdomain)
    assert!(!matches_pattern(
        "https://example.com/admin/users",
        "*.example.com/*"
    ));
}

#[test]
fn test_is_excluded() {
    // Note: Patterns match HOSTS, not paths (SSRF-safe design)
    // Use domain-based patterns for exclusion
    let patterns = vec![
        "*.admin.com".to_string(),
        "*.private.com".to_string(),
        "*.example.com".to_string(), // Exclude all example.com subdomains
    ];

    // Should be excluded: matches domain patterns
    assert!(is_excluded("https://admin.admin.com/page", &patterns));
    assert!(is_excluded("https://private.private.com/data", &patterns));
    assert!(is_excluded("https://blog.example.com/login", &patterns));

    // Should NOT be excluded: different domain
    assert!(!is_excluded("https://public.com/page", &patterns));

    // Note: example.com (no subdomain) does NOT match *.example.com
    // This is intentional - use "example.com" pattern to match the root domain
    assert!(!is_excluded("https://example.com/admin/users", &patterns));
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
}

/// Test crawl with a real small website
///
/// This test is ignored by default to avoid network calls.
/// Run with: `cargo test --test crawler_integration -- --ignored`
#[tokio::test]
#[ignore = "requires network"]
async fn test_crawl_site_small() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::builder(seed)
        .max_depth(1)
        .max_pages(5)
        .delay_ms(500)
        .build();

    let result = crawl_site(config).await.unwrap();

    assert!(result.total_pages >= 1);
    assert!(!result.urls.is_empty());
    println!("Crawled {} pages", result.total_pages);
}

/// Test URL discovery
///
/// This test is ignored by default to avoid network calls.
#[tokio::test]
#[ignore = "requires network"]
async fn test_discover_urls() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::new(seed);

    // FIX: Use discover_urls_for_tui instead of deprecated discover_urls
    // Note: discover_urls_for_tui doesn't take depth parameter
    let urls: Vec<_> = discover_urls_for_tui("https://example.com", &config)
        .await
        .unwrap();

    // example.com should have at least some links
    assert!(!urls.is_empty());
    println!("Discovered {} URLs", urls.len());
}

/// Test sitemap fetching
///
/// This test is ignored by default to avoid network calls.
#[tokio::test]
#[ignore = "requires network"]
async fn test_crawl_with_sitemap() {
    use webfang::crawl_with_sitemap;

    // Try with a site known to have a sitemap
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::new(seed);
    let urls: Vec<_> = crawl_with_sitemap("https://example.com", None, &config)
        .await
        .unwrap();

    // May be empty if no sitemap exists
    println!("Found {} URLs from sitemap", urls.len());
}

/// Test URL filtering with config
#[test]
fn test_is_allowed_complex() {
    let seed = Url::parse("https://example.com").unwrap();

    // Config with include and exclude patterns
    // Note: Patterns match HOSTS (SSRF-safe), not paths
    // *.domain = subdomains ONLY, domain = exact host
    let config = CrawlerConfig::builder(seed)
        // Include blog.example.com AND its subdomains
        .include_pattern("blog.example.com".to_string())
        .include_pattern("*.blog.example.com".to_string())
        // Include docs.example.com AND its subdomains
        .include_pattern("docs.example.com".to_string())
        .include_pattern("*.docs.example.com".to_string())
        // Exclude draft.example.com AND its subdomains
        .exclude_pattern("draft.example.com".to_string())
        .exclude_pattern("*.draft.example.com".to_string())
        .build();

    // Allowed: matches blog include (exact host)
    assert!(is_allowed("https://blog.example.com/post", &config));

    // Allowed: matches blog include (subdomain)
    assert!(is_allowed("https://news.blog.example.com/article", &config));

    // Allowed: matches docs include (exact host)
    assert!(is_allowed("https://docs.example.com/guide", &config));

    // Denied: matches draft exclude (exact host)
    assert!(!is_allowed("https://draft.example.com/post", &config));

    // Denied: matches draft exclude (subdomain)
    assert!(!is_allowed("https://test.draft.example.com/guide", &config));

    // Denied: doesn't match any include pattern
    assert!(!is_allowed("https://example.com/shop/products", &config));
    assert!(!is_allowed("https://admin.example.com/users", &config));
}

/// Test crawler config builder
#[test]
fn test_crawler_config_builder() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::builder(seed)
        .max_depth(5)
        .max_pages(500)
        .concurrency(5)
        .delay_ms(1000)
        .include_pattern("*.example.com/*".to_string())
        .exclude_pattern("*/admin/*".to_string())
        .user_agent("test-crawler/1.0")
        .timeout_secs(60)
        .build();

    assert_eq!(config.max_depth, 5);
    assert_eq!(config.max_pages, 500);
    assert_eq!(config.concurrency, 5);
    assert_eq!(config.delay_ms, 1000);
    assert_eq!(config.include_patterns.len(), 1);
    assert_eq!(config.exclude_patterns.len(), 1);
    assert_eq!(config.user_agent, "test-crawler/1.0");
    assert_eq!(config.timeout_secs, 60);
}

/// Test crawler config default values
#[test]
fn test_crawler_config_defaults() {
    let seed = Url::parse("https://example.com").unwrap();
    let config = CrawlerConfig::new(seed);

    assert_eq!(config.max_depth, 3);
    assert_eq!(config.max_pages, 100);
    assert_eq!(config.concurrency, 3); // Hardware-aware default
    assert_eq!(config.delay_ms, 500); // Hardware-aware default
}
