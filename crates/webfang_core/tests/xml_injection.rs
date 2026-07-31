//! Security fuzzing tests for XML / sitemap injection
//!
//! Verifies that the sitemap parser handles malicious inputs:
//! - Path traversal in <loc> elements
//! - Non-HTTP schemes in <loc>
//! - Max depth protection
//! - Content-Disposition path sanitization
//!
//! Note: Direct XML parsing tests use the public `resolve_url` function
//! and `SitemapParser::parse_from_url` (with wiremock for HTTP mocking).

use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ============================================================================
// URL resolution — path traversal in <loc> elements
// ============================================================================

fn resolve(base: &str, input: &str) -> Option<Url> {
    let base_url = Url::parse(base).unwrap();
    webfang::infrastructure::crawler::sitemap_parser::resolve_url(&base_url, input)
}

#[test]
fn path_traversal_in_loc_element() {
    let resolved = resolve("https://example.com/sitemap.xml", "../../../etc/passwd");
    if let Some(url) = resolved {
        let host = url.host_str().unwrap_or("");
        assert_eq!(host, "example.com");
    }
}

#[test]
fn absolute_path_traversal_not_resolved() {
    let resolved = resolve("https://example.com/sitemap.xml", "/../../../etc/passwd");
    if let Some(url) = resolved {
        assert_eq!(url.path(), "/etc/passwd");
    }
}

#[test]
fn protocol_relative_url() {
    let resolved = resolve("https://example.com/sitemap.xml", "//evil.com/steal");
    assert!(resolved.is_some());
    let url = resolved.unwrap();
    assert_eq!(url.host_str(), Some("evil.com"));
}

#[test]
fn javascript_scheme_in_loc() {
    let resolved = resolve("https://example.com/sitemap.xml", "javascript:alert(1)");
    if let Some(url) = resolved {
        assert!(
            url.scheme() != "http" && url.scheme() != "https",
            "javascript: should not resolve to HTTP"
        );
    }
}

#[test]
fn data_uri_in_loc() {
    let resolved = resolve(
        "https://example.com/sitemap.xml",
        "data:text/html,<script>alert(1)</script>",
    );
    // resolve_url resolves data: URIs as absolute URLs.
    // Downstream UrlValidator must filter non-HTTP schemes.
    if let Some(url) = resolved {
        assert_ne!(url.scheme(), "http", "data URI must not resolve to HTTP");
        assert_ne!(url.scheme(), "https", "data URI must not resolve to HTTPS");
    }
}

#[test]
fn empty_input_returns_none() {
    let resolved = resolve("https://example.com/sitemap.xml", "");
    assert!(resolved.is_none());
}

#[test]
fn whitespace_only_returns_none() {
    let resolved = resolve("https://example.com/sitemap.xml", "   ");
    assert!(resolved.is_none());
}

#[test]
fn relative_path_resolves_correctly() {
    let resolved = resolve("https://example.com/sitemap.xml", "/page");
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().path(), "/page");
}

#[test]
fn dotdot_resolves_within_domain() {
    let resolved = resolve("https://example.com/a/b/sitemap.xml", "../page");
    assert!(resolved.is_some());
    assert_eq!(resolved.unwrap().path(), "/a/page");
}

// ============================================================================
// Sitemap integration via parse_from_url with wiremock
// ============================================================================

#[tokio::test]
async fn parse_sitemap_with_path_traversal_locs() {
    let mock_server = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/valid</loc></url>
  <url><loc>ftp://evil.com/file</loc></url>
  <url><loc>file:///etc/passwd</loc></url>
  <url><loc>javascript:alert(1)</loc></url>
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock_server)
        .await;

    let parser = webfang::infrastructure::crawler::sitemap_parser::SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock_server.uri()))
        .await;

    if let Ok(urls) = result {
        // Only http/https URLs should be returned
        for url in &urls {
            assert!(
                url.scheme() == "http" || url.scheme() == "https",
                "Non-HTTP URL should be filtered: {url}"
            );
        }
    }
}

#[tokio::test]
async fn parse_sitemap_with_xxe_attempt() {
    let mock_server = MockServer::start().await;

    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE foo [
  <!ENTITY xxe SYSTEM "file:///etc/passwd">
]>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>&xxe;</loc></url>
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(xml.to_vec())
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock_server)
        .await;

    let parser = webfang::infrastructure::crawler::sitemap_parser::SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock_server.uri()))
        .await;

    if let Ok(urls) = result {
        for url in &urls {
            assert!(
                url.scheme() == "http" || url.scheme() == "https",
                "XXE should not produce non-HTTP URLs: {url}"
            );
        }
    }
}

#[tokio::test]
async fn parse_sitemap_billion_laughs_no_oom() {
    let mock_server = MockServer::start().await;

    let xml = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE lolz [
  <!ENTITY lol "lol">
  <!ENTITY lol2 "&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;&lol;">
  <!ENTITY lol3 "&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;&lol2;">
  <!ENTITY lol4 "&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;&lol3;">
]>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/&lol4;</loc></url>
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(xml.to_vec())
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock_server)
        .await;

    let parser = webfang::infrastructure::crawler::sitemap_parser::SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock_server.uri()))
        .await;

    // Should handle gracefully — no OOM, no panic
    // Entity expansion must not produce excessively large URLs
    if let Ok(urls) = &result {
        for url in urls {
            assert!(
                url.as_str().len() < 1_000_000,
                "billion laughs produced excessively long URL: {} chars",
                url.as_str().len()
            );
        }
    }
}

#[tokio::test]
async fn parse_sitemap_empty_xml_errors() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("")
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock_server)
        .await;

    let parser = webfang::infrastructure::crawler::sitemap_parser::SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock_server.uri()))
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn parse_sitemap_malformed_xml_no_panic() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    r#"<?xml version="1.0"?><urlset><url><loc>https://example.com/page</loc>"#,
                )
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock_server)
        .await;

    let parser = webfang::infrastructure::crawler::sitemap_parser::SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock_server.uri()))
        .await;

    // Should either parse or error — no panic
    let _ = result;
}

#[tokio::test]
async fn parse_sitemap_non_xml_content_type_with_xml_url() {
    // FINDING: The content-type check is bypassed when the URL ends in .xml.
    // This means a server returning HTML with a .xml URL will still be parsed.
    // This is by design (URL-based heuristic for servers that don't set content-type).
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>Not XML</body></html>")
                .insert_header("content-type", "text/html"),
        )
        .mount(&mock_server)
        .await;

    let parser = webfang::infrastructure::crawler::sitemap_parser::SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock_server.uri()))
        .await;

    // URL ends in .xml → parser accepts based on URL heuristic, not content-type.
    // The HTML body will fail XML parsing, so this should error.
    assert!(
        result.is_err(),
        "Non-XML body should fail XML parsing even if URL ends in .xml"
    );
}

#[tokio::test]
async fn parse_sitemap_non_xml_content_type_with_non_xml_url() {
    // When URL does NOT end in .xml AND content-type is text/html, it should be rejected.
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/sitemap"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>Not XML</body></html>")
                .insert_header("content-type", "text/html"),
        )
        .mount(&mock_server)
        .await;

    let parser = webfang::infrastructure::crawler::sitemap_parser::SitemapParser::new();
    let result = parser
        .parse_from_url(&format!("{}/sitemap", mock_server.uri()))
        .await;

    assert!(
        matches!(
            result,
            Err(webfang::infrastructure::crawler::sitemap_parser::SitemapError::InvalidContentType(_))
        ),
        "Non-XML content type with non-.xml URL should be rejected"
    );
}

// ============================================================================
// Max depth protection
// ============================================================================

#[tokio::test]
async fn max_depth_zero_returns_error() {
    use webfang::infrastructure::crawler::sitemap_config::SitemapConfig;

    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<urlset/>")
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock_server)
        .await;

    let config = SitemapConfig::builder().max_depth(0).build();
    assert_eq!(config.max_depth, 0);

    let parser =
        webfang::infrastructure::crawler::sitemap_parser::SitemapParser::with_config(config);
    let result = parser
        .parse_from_url(&format!("{}/sitemap.xml", mock_server.uri()))
        .await;
    assert!(matches!(
        result,
        Err(webfang::infrastructure::crawler::sitemap_parser::SitemapError::MaxDepthExceeded)
    ));
}

// ============================================================================
// Content-Disposition header injection via path sanitization
// ============================================================================

#[test]
fn path_traversal_in_filename_flat_no_risk() {
    // FINDING: sanitize_path_segment does NOT strip dots or '..'.
    // The path `/download/../../etc/passwd` becomes `download-..-..-etc-passwd.md`.
    // This is safe because the output is a flat filename (no directory separators),
    // so there's no actual filesystem traversal risk.
    use webfang::adapters::url_path::UrlPath;

    let path = UrlPath::from_url_path("/download/../../etc/passwd");
    let filename = path.to_safe_filename();
    assert!(
        !filename.contains('/'),
        "Filename should be flat: {filename}"
    );
}

#[test]
fn null_byte_in_filename_sanitized() {
    use webfang::adapters::url_path::UrlPath;

    let path = UrlPath::from_url_path("/download/file%00.pdf");
    let filename = path.to_safe_filename();
    assert!(
        !filename.contains('\0'),
        "Filename should not contain null bytes: {filename}"
    );
}

#[test]
fn very_long_filename_no_panic() {
    use webfang::adapters::url_path::UrlPath;

    let long_name = "a".repeat(1000);
    let path = UrlPath::from_url_path(&format!("/{long_name}"));
    let filename = path.to_safe_filename();
    assert!(!filename.is_empty());
}

#[test]
fn windows_reserved_names_in_filename() {
    use webfang::adapters::url_path::UrlPath;

    for reserved in &["CON", "PRN", "AUX", "NUL", "COM1", "LPT1"] {
        let path = UrlPath::from_url_path(&format!("/{reserved}"));
        let filename = path.to_safe_filename();
        assert!(
            filename.contains("_safe"),
            "Reserved name {reserved} should get _safe suffix: {filename}"
        );
    }
}

#[test]
fn special_chars_in_filename_sanitized() {
    use webfang::adapters::url_path::UrlPath;

    let path = UrlPath::from_url_path("/docs/page<with>special|chars");
    let filename = path.to_safe_filename();
    assert!(!filename.contains('<'));
    assert!(!filename.contains('>'));
    assert!(!filename.contains('|'));
}
