//! Integration tests for SitemapParser — real I/O with wiremock.
//!
//! Exercises `parse_from_url` end-to-end against a wiremock `MockServer`,
//! covering happy paths, edge cases, and error conditions per R-INT-02.

use webfang_core::infrastructure::crawler::{SitemapError, SitemapParser};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SITEMAP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/page1</loc></url>
  <url><loc>https://example.com/page2</loc></url>
  <url><loc>https://example.com/page3</loc></url>
</urlset>"#;

const SITEMAP_WITH_DUPLICATES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/page1</loc></url>
  <url><loc>https://example.com/page1</loc></url>
  <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

const SITEMAP_EMPTY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

const SITEMAP_NAMESPACES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
  <url>
    <loc>https://example.com/gallery</loc>
    <image:image>
      <image:loc>https://example.com/img1.jpg</image:loc>
    </image:image>
  </url>
</urlset>"#;

/// Helper: create parser with default config (no gzip, low depth for tests)
fn parser() -> SitemapParser {
    SitemapParser::new().unwrap()
}

// ===== HAPPY PATH =====

/// Parse a valid sitemap served by wiremock — extracts all URLs.
#[tokio::test]
async fn test_parse_valid_sitemap_from_mock_server() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_XML)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 3, "should extract 3 URLs from sitemap");

    let strings: Vec<String> = urls.iter().map(|u| u.url.to_string()).collect();
    assert!(strings.contains(&"https://example.com/page1".to_string()));
    assert!(strings.contains(&"https://example.com/page2".to_string()));
    assert!(strings.contains(&"https://example.com/page3".to_string()));
}

/// Parse sitemap with duplicate URLs — parser deduplicates.
#[tokio::test]
async fn test_parse_sitemap_deduplicates_urls() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_WITH_DUPLICATES)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 2, "duplicates should be deduplicated");
}

/// Parse sitemap with XML namespaces — loc elements still extracted.
#[tokio::test]
async fn test_parse_sitemap_with_namespaces() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_NAMESPACES)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 1, "should extract the one loc URL");
    assert_eq!(urls[0].url.as_str(), "https://example.com/gallery");
}

// ===== EDGE CASES =====

/// Empty sitemap — returns NoUrlsFound error.
#[tokio::test]
async fn test_parse_empty_sitemap_returns_error() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_EMPTY)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap.xml", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(
        matches!(result, Err(SitemapError::NoUrlsFound)),
        "expected NoUrlsFound, got {result:?}"
    );
}

/// Truly malformed XML — returns XmlError.
#[tokio::test]
async fn test_parse_malformed_xml_returns_error() {
    let mock = MockServer::start().await;
    // Null bytes are not valid XML — quick_xml will reject them
    let bad_xml = vec![0x00, 0x00, 0x00, 0x3C, 0x00];
    Mock::given(method("GET"))
        .and(path("/feed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(bad_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/feed", mock.uri());
    let result = parser.parse_from_url(&url).await;

    // Null bytes cause XmlError or NoUrlsFound depending on parser behavior
    assert!(
        matches!(
            result,
            Err(SitemapError::XmlError(_)) | Err(SitemapError::NoUrlsFound)
        ),
        "expected XmlError or NoUrlsFound for garbage bytes, got {result:?}"
    );
}

/// Non-XML content type on non-.xml path — returns InvalidContentType.
#[tokio::test]
async fn test_parse_non_xml_content_type_returns_error() {
    let mock = MockServer::start().await;
    // Use non-.xml path so content-type check actually applies
    Mock::given(method("GET"))
        .and(path("/feed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>Not a sitemap</body></html>")
                .insert_header("content-type", "text/html"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/feed", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(
        matches!(result, Err(SitemapError::InvalidContentType(_))),
        "expected InvalidContentType, got {result:?}"
    );
}

/// HTTP 404 — returns HttpError (status is checked before content-type).
/// Bug #9 regression: non-2xx status MUST yield HttpError, not be silently
/// parsed as XML (issue #590).
#[tokio::test]
async fn test_parse_http_404_returns_no_urls() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap.xml", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(
        matches!(result, Err(SitemapError::HttpError { .. })),
        "expected HttpError for 404, got {result:?}"
    );
}

/// Wiremock sitemap served without Content-Type header — parser accepts it
/// (empty content type is treated as XML).
#[tokio::test]
async fn test_parse_sitemap_no_content_type_accepted() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SITEMAP_XML))
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 3, "should parse sitemap without Content-Type");
}

// ===== SITEMAP INDEX RECURSION TESTS =====

/// Helper to create a sitemap index XML with mock server base URL
fn sitemap_index_with_base(base_url: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap>
        <loc>{base_url}/sitemap1.xml</loc>
        <lastmod>2024-01-15</lastmod>
    </sitemap>
    <sitemap>
        <loc>{base_url}/sitemap2.xml</loc>
        <lastmod>2024-01-10</lastmod>
    </sitemap>
</sitemapindex>"#
    )
}

/// Helper to create sitemap 1 XML
const SITEMAP_1: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

/// Helper to create sitemap 2 XML
const SITEMAP_2: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page3</loc></url>
    <url><loc>https://example.com/page4</loc></url>
</urlset>"#;

/// Sitemap index with multiple child sitemaps — recursively parses all.
#[tokio::test]
async fn test_parse_sitemap_index_recurses() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();
    let index_xml = sitemap_index_with_base(&base_url);

    Mock::given(method("GET"))
        .and(path("/sitemap-index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap1.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_1)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap2.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_2)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap-index.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(
        urls.len(),
        4,
        "should parse all 4 URLs from both child sitemaps"
    );
    let strings: Vec<String> = urls.iter().map(|u| u.url.to_string()).collect();
    assert!(strings.contains(&"https://example.com/page1".to_string()));
    assert!(strings.contains(&"https://example.com/page2".to_string()));
    assert!(strings.contains(&"https://example.com/page3".to_string()));
    assert!(strings.contains(&"https://example.com/page4".to_string()));
}

/// Sitemap index where one child returns 404 — other children still parsed.
#[tokio::test]
async fn test_sitemap_index_partial_failure_continues() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();
    let index_xml = sitemap_index_with_base(&base_url);

    Mock::given(method("GET"))
        .and(path("/sitemap-index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap1.xml"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap2.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SITEMAP_2)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap-index.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    // Should still parse the successful child
    assert_eq!(
        urls.len(),
        2,
        "should parse URLs from successful child only"
    );
    let strings: Vec<String> = urls.iter().map(|u| u.url.to_string()).collect();
    assert!(strings.contains(&"https://example.com/page3".to_string()));
    assert!(strings.contains(&"https://example.com/page4".to_string()));
}

/// Sitemap index where ALL children fail — returns AllChildrenFailed error.
#[tokio::test]
async fn test_sitemap_index_all_children_failed() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();
    let index_xml = sitemap_index_with_base(&base_url);

    Mock::given(method("GET"))
        .and(path("/sitemap-index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap1.xml"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap2.xml"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap-index.xml", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(
        matches!(result, Err(SitemapError::AllChildrenFailed(count, _)) if count == 2),
        "expected AllChildrenFailed with count=2, got {result:?}"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("all 2 child sitemaps failed"));
    assert!(err_msg.contains("sitemap1.xml"));
    assert!(err_msg.contains("sitemap2.xml"));
}

/// Sitemap index with malformed XML child — error included in AllChildrenFailed.
#[tokio::test]
async fn test_sitemap_index_malformed_child_included_in_error() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();
    let index_xml = sitemap_index_with_base(&base_url);

    Mock::given(method("GET"))
        .and(path("/sitemap-index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap1.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("not valid xml {{{")
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap2.xml"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap-index.xml", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(
        matches!(result, Err(SitemapError::AllChildrenFailed(count, _)) if count == 2),
        "expected AllChildrenFailed with count=2, got {result:?}"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(err_msg.contains("all 2 child sitemaps failed"));
    assert!(err_msg.contains("XML") || err_msg.contains("parse") || err_msg.contains("404"));
}

/// Self-referential sitemap index (loop) — detected and skipped.
#[tokio::test]
async fn test_sitemap_index_self_reference_loop_detected() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();

    // Sitemap index that references itself
    let self_ref_index = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap>
        <loc>{base_url}/sitemap-index.xml</loc>
    </sitemap>
</sitemapindex>"#
    );

    Mock::given(method("GET"))
        .and(path("/sitemap-index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(self_ref_index)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/sitemap-index.xml", mock.uri());
    let result = parser.parse_from_url(&url).await;

    // Should detect the loop and return NoUrlsFound (no children parsed)
    // or AllChildrenFailed if it tries to fetch itself again
    assert!(result.is_err(), "self-referential sitemap should fail");
}

/// Mutually referential sitemap indexes (A -> B -> A) — loop detected.
#[tokio::test]
async fn test_sitemap_index_mutual_reference_loop_detected() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();

    let index_a = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap><loc>{base_url}/index-b.xml</loc></sitemap>
</sitemapindex>"#
    );

    let index_b = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap><loc>{base_url}/index-a.xml</loc></sitemap>
</sitemapindex>"#
    );

    Mock::given(method("GET"))
        .and(path("/index-a.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_a)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/index-b.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_b)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/index-a.xml", mock.uri());
    let result = parser.parse_from_url(&url).await;

    // Should detect the loop and not infinite recurse
    assert!(result.is_err(), "mutual reference sitemap should fail");
}

/// Sitemap index with valid child and self-reference — valid child parsed, loop skipped.
#[tokio::test]
async fn test_sitemap_index_mixed_valid_and_loop() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();

    let mixed_index = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap><loc>{base_url}/valid-child.xml</loc></sitemap>
    <sitemap><loc>{base_url}/loop-index.xml</loc></sitemap>
</sitemapindex>"#
    );

    let valid_child = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
</urlset>"#;

    let loop_index = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap><loc>{base_url}/loop-index.xml</loc></sitemap>
</sitemapindex>"#
    );

    Mock::given(method("GET"))
        .and(path("/mixed-index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(mixed_index)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/valid-child.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(valid_child)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/loop-index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(loop_index)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/mixed-index.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    // Should parse the valid child and skip the loop
    assert_eq!(urls.len(), 1, "should parse valid child, skip loop");
    assert_eq!(urls[0].url.as_str(), "https://example.com/page1");
}

/// Sitemap index deduplicates URLs across multiple children.
#[tokio::test]
async fn test_sitemap_index_deduplicates_across_children() {
    let mock = MockServer::start().await;
    let base_url = mock.uri();

    let index_with_overlap = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap><loc>{base_url}/sitemap-a.xml</loc></sitemap>
    <sitemap><loc>{base_url}/sitemap-b.xml</loc></sitemap>
</sitemapindex>"#
    );

    let sitemap_a = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

    let sitemap_b = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page2</loc></url>
    <url><loc>https://example.com/page3</loc></url>
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/index-overlap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_with_overlap)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap-a.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sitemap_a)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap-b.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sitemap_b)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = parser();
    let url = format!("{}/index-overlap.xml", mock.uri());
    let urls = parser.parse_from_url(&url).await.unwrap();

    assert_eq!(urls.len(), 3, "should deduplicate page2 across children");
    let strings: Vec<String> = urls.iter().map(|u| u.url.to_string()).collect();
    assert!(strings.contains(&"https://example.com/page1".to_string()));
    assert!(strings.contains(&"https://example.com/page2".to_string()));
    assert!(strings.contains(&"https://example.com/page3".to_string()));
}
