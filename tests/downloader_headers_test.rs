//! Integration test: [`WreqDownloader`] populates `FetchedPage.headers`.
//!
//! Verifies the header-capture path added in the JsStrategy wiring (#303):
//! response headers from the wire are lowercased and available in
//! `FetchedPage.headers` for downstream content-type sniffing.

#[path = "common/cli_harness.rs"]
mod common;

use url::Url;
use webfang_core::infrastructure::downloader::wreq_downloader::WreqDownloader;
use webfang_core::infrastructure::downloader::Downloader;
use webfang_core::HttpClientConfig;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
#[cfg(not(miri))]
async fn wreq_downloader_populates_response_headers() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw("<html><body>ok</body></html>", "text/html; charset=utf-8")
                .insert_header("X-Custom-Header", "test-value-42"),
        )
        .mount(&server)
        .await;

    let downloader = WreqDownloader::new(10, 5, HttpClientConfig::default().tls_emulation)
        .expect("downloader builds");
    let url = Url::parse(&server.uri()).expect("wiremock URI is valid");

    let page = downloader
        .fetch(&url)
        .await
        .expect("fetch from wiremock should succeed");

    assert_eq!(page.status, 200);

    let ct = page
        .headers
        .get("content-type")
        .expect("content-type header must be present");
    assert!(
        ct.contains("text/html"),
        "content-type should contain text/html, got: {ct}"
    );

    let custom = page
        .headers
        .get("x-custom-header")
        .expect("custom header must be present (lowercased key)");
    assert_eq!(custom, "test-value-42");
}

#[tokio::test]
#[cfg(not(miri))]
async fn wreq_downloader_headers_keys_are_lowercased() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("ok")
                .insert_header("X-Mixed-Case", "value"),
        )
        .mount(&server)
        .await;

    let downloader = WreqDownloader::new(10, 5, HttpClientConfig::default().tls_emulation)
        .expect("downloader builds");
    let url = Url::parse(&server.uri()).expect("wiremock URI is valid");

    let page = downloader.fetch(&url).await.expect("fetch should succeed");

    assert!(
        page.headers.contains_key("x-mixed-case"),
        "header keys must be lowercased; got keys: {:?}",
        page.headers.keys().collect::<Vec<_>>()
    );
    assert!(
        !page.headers.contains_key("X-Mixed-Case"),
        "original mixed-case key must not appear"
    );
}
