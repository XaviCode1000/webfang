//! Orchestrator integration tests — public interface contracts.
//!
//! Tests sitemap integration via `SitemapParser` (public API).
//! `build_elastic_ingestion` and `plan_urls` are tested inline in
//! `orchestrator.rs` since they are private functions.
//!
//! Following contract-based-test-audit: observable behavior only, wiremock for HTTP.

use wiremock::{MockServer, ResponseTemplate};

use webfang_test_utils::EnvGuard;

// The serve→parse arrange is shared with the other two sitemap test binaries
// of this crate (`sitemap_integration.rs`, `sitemap_behavioral.rs`): it lives
// in the crate's shared test module, not inside a single binary (#1757, S1).
#[path = "common/mod.rs"]
mod common;
use common::sitemap_arrange::serve_and_parse;

// One-layer entry disarmer for the loopback mock (#1382/#1369), canonical in
// `EnvGuard::entry_guard_off` since #1396: these smoke tests pin the public
// parser API against wiremock, not the entry guard (pinned separately in
// `sitemap_ssrf_e2e_test`).

// ===========================================================================
// Sitemap integration via SitemapParser (smoke tests)
// ===========================================================================

/// Sitemap served by wiremock is parsed and URLs are extracted.
#[tokio::test]
async fn sitemap_valid_xml_discovers_urls() {
    let _entry_off = EnvGuard::entry_guard_off();
    let mock = MockServer::start().await;

    let sitemap_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

    let urls = serve_and_parse(
        &mock,
        "/sitemap.xml",
        ResponseTemplate::new(200)
            .set_body_string(sitemap_xml)
            .insert_header("content-type", "application/xml"),
        "/sitemap.xml",
    )
    .await
    .unwrap();

    assert_eq!(urls.len(), 2, "should discover 2 URLs from sitemap");
    let strings: Vec<String> = urls.iter().map(|u| u.url.to_string()).collect();
    assert!(strings.contains(&"https://example.com/page1".to_string()));
    assert!(strings.contains(&"https://example.com/page2".to_string()));
}

/// Malformed sitemap XML returns an error gracefully.
#[tokio::test]
async fn sitemap_malformed_xml_returns_error() {
    let mock = MockServer::start().await;

    let bad_xml = vec![0x00, 0x00, 0x00, 0x3C, 0x00];

    let result = serve_and_parse(
        &mock,
        "/bad-sitemap",
        ResponseTemplate::new(200)
            .set_body_bytes(bad_xml)
            .insert_header("content-type", "application/xml"),
        "/bad-sitemap",
    )
    .await;

    assert!(result.is_err(), "malformed XML should produce an error");
}

/// Large sitemap (200 URLs) is parsed without error.
#[tokio::test]
async fn sitemap_large_sitemap_parses_all_urls() {
    let _entry_off = EnvGuard::entry_guard_off();
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

    let urls = serve_and_parse(
        &mock,
        "/big-sitemap.xml",
        ResponseTemplate::new(200)
            .set_body_string(&xml)
            .insert_header("content-type", "application/xml"),
        "/big-sitemap.xml",
    )
    .await
    .unwrap();

    assert_eq!(urls.len(), 200, "should extract all 200 URLs");
}

/// Empty sitemap returns NoUrlsFound error.
#[tokio::test]
async fn sitemap_empty_returns_no_urls_found() {
    let _entry_off = EnvGuard::entry_guard_off();
    let mock = MockServer::start().await;

    let empty_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

    let result = serve_and_parse(
        &mock,
        "/empty.xml",
        ResponseTemplate::new(200)
            .set_body_string(empty_xml)
            .insert_header("content-type", "application/xml"),
        "/empty.xml",
    )
    .await;

    assert!(
        matches!(
            result,
            Err(webfang_core::infrastructure::crawler::SitemapError::NoUrlsFound)
        ),
        "expected NoUrlsFound, got {result:?}"
    );
}

/// Sitemap with duplicate URLs is deduplicated.
#[tokio::test]
async fn sitemap_deduplicates_urls() {
    let _entry_off = EnvGuard::entry_guard_off();
    let mock = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

    let urls = serve_and_parse(
        &mock,
        "/sitemap.xml",
        ResponseTemplate::new(200)
            .set_body_string(xml)
            .insert_header("content-type", "application/xml"),
        "/sitemap.xml",
    )
    .await
    .unwrap();

    assert_eq!(urls.len(), 2, "duplicates should be deduplicated");
}
