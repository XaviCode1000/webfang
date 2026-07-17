//! Integration tests for sitemap_parser — XML fixture parsing and wiremock HTTP.
//!
//! Parses real XML fixtures from tests/fixtures/sitemap/ and exercises
//! SitemapParser + SitemapConfig against wiremock MockServer.

use webfang::infrastructure::crawler::SitemapConfig;
use webfang::infrastructure::crawler::{resolve_url, SitemapError, SitemapParser};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── XML fixture helpers ───────────────────────────────────────────────────

fn fixture(name: &str) -> String {
    let path = format!("tests/fixtures/sitemap/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read fixture {path}: {e}"))
}

// ── SitemapConfig defaults ────────────────────────────────────────────────

#[test]
fn config_defaults_are_sane() {
    let config = SitemapConfig::default();
    assert!(config.gzip_enabled);
    assert_eq!(config.max_depth, 3);
    assert_eq!(config.concurrency, 5);
    assert!(config.max_response_size > 0);
    assert!(config.max_decompressed_size > 0);
}

#[test]
fn config_builder_overrides_defaults() {
    let config = SitemapConfig::builder()
        .gzip_enabled(false)
        .max_depth(7)
        .concurrency(20)
        .max_response_size(1024)
        .max_decompressed_size(2048)
        .build();

    assert!(!config.gzip_enabled);
    assert_eq!(config.max_depth, 7);
    assert_eq!(config.concurrency, 20);
    assert_eq!(config.max_response_size, 1024);
    assert_eq!(config.max_decompressed_size, 2048);
}

#[test]
fn config_zero_falls_back_to_defaults() {
    let config = SitemapConfig::builder()
        .max_response_size(0)
        .max_decompressed_size(0)
        .build();
    assert_eq!(config.max_response_size, 52_428_800);
    assert_eq!(config.max_decompressed_size, 104_857_600);
}

// ── resolve_url ───────────────────────────────────────────────────────────

#[test]
fn resolve_url_absolute_passthrough() {
    let base = Url::parse("https://example.com/sitemap.xml").unwrap();
    let resolved = resolve_url(&base, "https://other.com/page").unwrap();
    assert_eq!(resolved.as_str(), "https://other.com/page");
}

#[test]
fn resolve_url_relative_path() {
    let base = Url::parse("https://example.com/sitemap.xml").unwrap();
    assert_eq!(
        resolve_url(&base, "/page").unwrap().as_str(),
        "https://example.com/page"
    );
    assert_eq!(
        resolve_url(&base, "page.html").unwrap().as_str(),
        "https://example.com/page.html"
    );
}

#[test]
fn resolve_url_empty_returns_none() {
    let base = Url::parse("https://example.com").unwrap();
    assert!(resolve_url(&base, "").is_none());
    assert!(resolve_url(&base, "   ").is_none());
}

#[test]
fn resolve_url_parent_directory() {
    let base = Url::parse("https://example.com/a/b/sitemap.xml").unwrap();
    let resolved = resolve_url(&base, "../page").unwrap();
    assert_eq!(resolved.as_str(), "https://example.com/a/page");
}

// ── Wiremock HTTP tests ───────────────────────────────────────────────────

#[tokio::test]
async fn parse_from_mock_server_basic_sitemap() {
    let mock = MockServer::start().await;
    let xml = fixture("basic.xml");
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(&xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = SitemapParser::new();
    let urls = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
        .await
        .unwrap();

    assert_eq!(urls.len(), 3);
}

#[tokio::test]
async fn parse_from_mock_server_empty_sitemap() {
    let mock = MockServer::start().await;
    let xml = fixture("empty.xml");
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(&xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
        .await;

    assert!(matches!(result, Err(SitemapError::NoUrlsFound)));
}

#[tokio::test]
async fn parse_from_mock_server_non_xml_content_type() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html>Not a sitemap</html>")
                .insert_header("content-type", "text/html"),
        )
        .mount(&mock)
        .await;

    let parser = SitemapParser::new();
    let result = parser.parse_from_url(&format!("{}/feed", mock.uri())).await;

    assert!(matches!(result, Err(SitemapError::InvalidContentType(_))));
}

#[tokio::test]
async fn parse_from_mock_server_404() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&mock)
        .await;

    let parser = SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
        .await;

    // 404 body is not valid XML, so parser returns NoUrlsFound
    assert!(matches!(result, Err(SitemapError::NoUrlsFound)));
}

#[tokio::test]
async fn parse_from_mock_server_no_content_type_accepted() {
    let mock = MockServer::start().await;
    let xml = fixture("basic.xml");
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&xml))
        .mount(&mock)
        .await;

    let parser = SitemapParser::new();
    let urls = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
        .await
        .unwrap();

    assert_eq!(urls.len(), 3);
}

#[tokio::test]
async fn parse_from_mock_server_deduplicates() {
    let mock = MockServer::start().await;
    let xml = fixture("duplicates.xml");
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(&xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = SitemapParser::new();
    let urls = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
        .await
        .unwrap();

    assert_eq!(urls.len(), 2);
}

#[tokio::test]
async fn parse_from_mock_server_filter_invalid_schemes() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
                    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
                        <url><loc>https://example.com/valid</loc></url>
                        <url><loc>http://example.com/valid</loc></url>
                        <url><loc>ftp://example.com/invalid</loc></url>
                        <url><loc>javascript:alert(1)</loc></url>
                    </urlset>"#,
                )
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = SitemapParser::new();
    let urls = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
        .await
        .unwrap();

    assert_eq!(urls.len(), 2);
    assert!(urls
        .iter()
        .all(|u| u.scheme() == "http" || u.scheme() == "https"));
}

#[tokio::test]
async fn parse_depth_zero_returns_error_without_network() {
    let config = SitemapConfig::builder().max_depth(0).build();
    let parser = SitemapParser::with_config(config);
    let result = parser
        .parse_from_url("https://example.com/sitemap.xml")
        .await;
    assert!(matches!(result, Err(SitemapError::MaxDepthExceeded)));
}

#[tokio::test]
async fn parser_has_gzip_accessor() {
    let gzip_on = SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(true).build());
    assert!(gzip_on.has_gzip());

    let gzip_off = SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(false).build());
    assert!(!gzip_off.has_gzip());
}

#[tokio::test]
async fn parser_max_depth_accessor() {
    let parser = SitemapParser::with_config(SitemapConfig::builder().max_depth(7).build());
    assert_eq!(parser.max_depth(), 7);
}
