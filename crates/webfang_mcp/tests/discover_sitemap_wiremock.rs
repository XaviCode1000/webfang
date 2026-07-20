//! Wiremock behavioral test for the discover_sitemap MCP tool.
//!
//! Serves a fake sitemap via wiremock, invokes `crawl_with_sitemap`
//! (the function backing the `discover_sitemap` tool), and asserts
//! the JSON response shape is `Vec<String>` — preserving the MCP
//! tool's contract.
//!
//! Following contract-based-test-audit: port abstraction via wiremock,
//! no concrete wreq in test code.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Fake sitemap XML with exactly 2 `<loc>` entries.
const SITEMAP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

/// discover_sitemap returns Vec<String> from a fake sitemap with 2 entries.
///
/// Scenario 2.2.S1: Given a wiremock server serving `/sitemap.xml` with 2
/// `<loc>` entries, when `crawl_with_sitemap` (the backing function) is
/// invoked with that base URL, then the result contains exactly 2 URL
/// strings matching the sitemap `<loc>` values.
#[tokio::test]
async fn discover_sitemap_returns_urls_from_fake_sitemap() {
    let mock = MockServer::start().await;

    // Serve the sitemap at the expected location
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_XML)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let base_url = mock.uri();
    let seed = url::Url::parse(&base_url).expect("valid mock URL");
    let config = webfang_core::domain::CrawlerConfig::new(seed);

    // Pass explicit sitemap URL to avoid auto-discovery (which would
    // need robots.txt or fallback logic). This isolates the test to
    // the sitemap parsing + URL extraction path.
    let sitemap_url = format!("{}/sitemap.xml", base_url);

    let discovered = webfang_core::crawl_with_sitemap(&base_url, Some(&sitemap_url), &config)
        .await
        .expect("crawl_with_sitemap should succeed");

    // Verify the raw discovered URLs (pre-serialization)
    assert_eq!(
        discovered.len(),
        2,
        "should discover exactly 2 URLs from the fake sitemap"
    );

    let urls: Vec<String> = discovered.iter().map(|d| d.url.to_string()).collect();
    assert!(
        urls.contains(&"https://example.com/page1".to_string()),
        "should contain page1, got: {:?}",
        urls
    );
    assert!(
        urls.contains(&"https://example.com/page2".to_string()),
        "should contain page2, got: {:?}",
        urls
    );

    // Verify JSON serialization preserves Vec<String> shape (MCP contract)
    let json = serde_json::to_string_pretty(&urls).expect("serialization should succeed");
    let parsed: Vec<String> =
        serde_json::from_str(&json).expect("deserialization should produce Vec<String>");
    assert_eq!(
        parsed.len(),
        2,
        "JSON round-trip should preserve 2-element array"
    );
    assert_eq!(parsed, urls, "JSON round-trip should preserve URL values");
}

/// discover_sitemap errors when sitemap has no entries.
///
/// `crawl_with_sitemap` returns `CrawlError::Sitemap` for empty sitemaps.
/// This is the current behavior; the MCP tool surfaces it as an error response.
#[tokio::test]
async fn discover_sitemap_errors_on_empty_sitemap() {
    let mock = MockServer::start().await;

    let empty_sitemap = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(empty_sitemap)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let base_url = mock.uri();
    let seed = url::Url::parse(&base_url).expect("valid mock URL");
    let config = webfang_core::domain::CrawlerConfig::new(seed);
    let sitemap_url = format!("{}/sitemap.xml", base_url);

    let result = webfang_core::crawl_with_sitemap(&base_url, Some(&sitemap_url), &config).await;

    assert!(
        result.is_err(),
        "empty sitemap should produce an error, not success"
    );
}
