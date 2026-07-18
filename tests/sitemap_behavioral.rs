//! Sitemap behavioral tests — end-to-end sitemap contracts.
//!
//! Tests the full sitemap pipeline: parse → discover URLs → validate.
//! Uses wiremock for HTTP mocking, no real network calls.
//!
//! Following contract-based-test-audit: observable behavior only, wiremock for HTTP.
//! Following the task spec: valid sitemap, malformed XML, large sitemap (1000+ URLs).

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Valid sitemap XML served by wiremock → discovers all URLs.
#[tokio::test]
async fn sitemap_valid_discovers_all_urls() {
    let mock = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>https://example.com/page1</loc></url>
    <url><loc>https://example.com/page2</loc></url>
    <url><loc>https://example.com/page3</loc></url>
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

    assert_eq!(urls.len(), 3, "should discover 3 URLs from valid sitemap");
    let strings: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
    assert!(strings.contains(&"https://example.com/page1".to_string()));
    assert!(strings.contains(&"https://example.com/page2".to_string()));
    assert!(strings.contains(&"https://example.com/page3".to_string()));
}

/// Malformed XML → graceful degradation (error returned, no panic).
#[tokio::test]
async fn sitemap_malformed_xml_graceful_degradation() {
    let mock = MockServer::start().await;

    // Null bytes — not valid XML
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

    assert!(
        result.is_err(),
        "malformed XML should produce an error, not panic"
    );
}

/// Partially malformed XML (missing closing tags) — parser handles gracefully.
#[tokio::test]
async fn sitemap_partially_malformed_xml() {
    let mock = MockServer::start().await;

    let partial_xml = r#"<?xml version="1.0"?>
<urlset>
    <url><loc>https://example.com/page1</loc>
    <!-- Missing closing tags -->
</urlset>"#;

    Mock::given(method("GET"))
        .and(path("/partial-sitemap"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(partial_xml)
                .insert_header("content-type", "application/xml"),
        )
        .mount(&mock)
        .await;

    let parser = webfang_core::infrastructure::crawler::SitemapParser::new();
    let url = format!("{}/partial-sitemap", mock.uri());
    let result = parser.parse_from_url(&url).await;

    // Should either parse what it can or return an error — never panic
    match result {
        Ok(urls) => assert!(!urls.is_empty(), "parsed URLs should not be empty"),
        Err(e) => assert!(
            format!("{}", e).contains("XML") || format!("{}", e).contains("no URLs"),
            "error should be XML-related: {e}"
        ),
    }
}

/// Large sitemap (1000+ URLs) → performance check (parses within reasonable time).
#[tokio::test]
async fn sitemap_large_1000_plus_urls() {
    let mock = MockServer::start().await;

    let mut xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#,
    );
    for i in 0..1500 {
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

    let start = std::time::Instant::now();
    let urls = parser.parse_from_url(&url).await.unwrap();
    let elapsed = start.elapsed();

    assert_eq!(urls.len(), 1500, "should extract all 1500 URLs");
    // Performance budget: 1500-URL sitemap should parse in under 2 seconds
    assert!(
        elapsed.as_secs() < 2,
        "parsing 1500 URLs took {:?}, expected < 2s",
        elapsed
    );
}

/// Empty sitemap → NoUrlsFound error.
#[tokio::test]
async fn sitemap_empty_returns_error() {
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

/// Sitemap with duplicate URLs → deduplicated.
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

/// Sitemap with XML namespaces → loc elements still extracted.
#[tokio::test]
async fn sitemap_with_namespaces() {
    let mock = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
    <url>
        <loc>https://example.com/gallery</loc>
        <image:image>
            <image:loc>https://example.com/img1.jpg</image:loc>
        </image:image>
    </url>
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

    assert!(
        !urls.is_empty(),
        "should extract at least 1 URL despite namespaces"
    );
    assert_eq!(urls[0].as_str(), "https://example.com/gallery");
}

/// Non-XML content type on non-.xml path → InvalidContentType error.
#[tokio::test]
async fn sitemap_non_xml_content_type_returns_error() {
    let mock = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/feed"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>Not a sitemap</body></html>")
                .insert_header("content-type", "text/html"),
        )
        .mount(&mock)
        .await;

    let parser = webfang_core::infrastructure::crawler::SitemapParser::new();
    let url = format!("{}/feed", mock.uri());
    let result = parser.parse_from_url(&url).await;

    assert!(
        matches!(
            result,
            Err(webfang_core::infrastructure::crawler::SitemapError::InvalidContentType(_))
        ),
        "expected InvalidContentType, got {:?}",
        result
    );
}
