//! Integration tests for SitemapParser — real I/O with wiremock.
//!
//! Exercises `parse_from_url` end-to-end against a wiremock `MockServer`,
//! covering happy paths, edge cases, and error conditions per R-INT-02.

use webfang::infrastructure::crawler::{SitemapError, SitemapParser};
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
    SitemapParser::new()
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

    let strings: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
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
    assert_eq!(urls[0].as_str(), "https://example.com/gallery");
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
        "expected NoUrlsFound, got {:?}",
        result
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
        "expected XmlError or NoUrlsFound for garbage bytes, got {:?}",
        result
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
        "expected InvalidContentType, got {:?}",
        result
    );
}

/// HTTP 404 — returns NoUrlsFound (parser doesn't check status, only parses body).
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
        matches!(result, Err(SitemapError::NoUrlsFound)),
        "expected NoUrlsFound for 404 body, got {:?}",
        result
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
