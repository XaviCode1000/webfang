//! Orchestrator integration tests — public interface contracts.
//!
//! Tests sitemap integration via `SitemapParser` (public API).
//! `build_elastic_ingestion` and `plan_urls` are tested inline in
//! `orchestrator.rs` since they are private functions.
//!
//! Following contract-based-test-audit: observable behavior only, wiremock for HTTP.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ===========================================================================
// Sitemap integration via SitemapParser (smoke tests)
// ===========================================================================

/// Sitemap served by wiremock is parsed and URLs are extracted.
#[tokio::test]
async fn sitemap_valid_xml_discovers_urls() {
    let mock = MockServer::start().await;

    let sitemap_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sitemap_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = webfang_core::infrastructure::crawler::SitemapParser::new();
    let url = format!("{}/sitemap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 2, "should discover 2 URLs from sitemap");
    let strings: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
    assert!(strings.contains(&"https://example.com/page1".to_string()));
    assert!(strings.contains(&"https://example.com/page2".to_string()));
}

/// Malformed sitemap XML returns an error gracefully.
#[tokio::test]
async fn sitemap_malformed_xml_returns_error() {
    let mock = MockServer::start().await;

    let bad_xml = vec![0x00, 0x00, 0x00, 0x3C, 0x00];
    Mock::given(method("GET"))
        .and(path("/bad-sitemap"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(bad_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = webfang_core::infrastructure::crawler::SitemapParser::new();
    let url = format!("{}/bad-sitemap", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(result.is_err(), "malformed XML should produce an error");
}

/// Large sitemap (200 URLs) is parsed without error.
#[tokio::test]
async fn sitemap_large_sitemap_parses_all_urls() {
    let mock = MockServer::start().await;

    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#,
    );
    for i in 0..200 {
        xml.push_str(&format!(
            "<url><loc>https://example.com/page{i}</loc></url>"
        ));
    }
    xml.push_str("</urlset>");

    Mock::given(method("GET"))
        .and(path("/big-sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(&xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = webfang_core::infrastructure::crawler::SitemapParser::new();
    let url = format!("{}/big-sitemap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 200, "should extract all 200 URLs");
}

/// Empty sitemap returns NoUrlsFound error.
#[tokio::test]
async fn sitemap_empty_returns_no_urls_found() {
    let mock = MockServer::start().await;

    let empty_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/empty.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(empty_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = webfang_core::infrastructure::crawler::SitemapParser::new();
    let url = format!("{}/empty.xml", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(
        matches!(
            result,
            Err(webfang_core::infrastructure::crawler::SitemapError::NoUrlsFound)
        ),
        "expected NoUrlsFound, got {:?}",
        result
    );
}

/// Sitemap with duplicate URLs is deduplicated.
#[tokio::test]
async fn sitemap_deduplicates_urls() {
    let mock = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = webfang_core::infrastructure::crawler::SitemapParser::new();
    let url = format!("{}/sitemap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 2, "duplicates should be deduplicated");
}
