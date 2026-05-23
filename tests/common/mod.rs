//! Shared test fixtures and helpers
//!
//! Common utilities for integration tests:
//! - HTML fixture loading
//! - WireMock server setup
//! - Test content generators
//!
//! # Usage
//!
//! ```ignore
//! mod common;
//! use common::{load_fixture, mock_server};
//! ```

#![cfg(not(miri))]

use std::path::Path;

/// TestHttpServer - RAII wrapper for wiremock MockServer
///
/// Provides deterministic HTTP mocking for integration tests.
/// Each test gets its own isolated server instance.
///
/// # Usage
///
/// ```ignore
/// #[tokio::test]
/// async fn test_http_client() {
///     let server = TestHttpServer::new().await;
///     let base_url = server.uri();
///
///     // Register a mock response
///     server.mock_response(
///         wiremock::matchers::method("GET"),
///         "/api/test",
///         200,
///         r#"{"status":"ok"}"#
///     ).await;
///
///     // Use base_url in your client
///     let client = HttpClient::new(&base_url);
///     let response = client.get("test").await;
///     assert!(response.is_ok());
/// }
/// ```
pub struct TestHttpServer {
    server: wiremock::MockServer,
    base_url: String,
}

impl TestHttpServer {
    /// Create a new mock server on a random available port.
    ///
    /// Each call gets a completely isolated server.
    pub async fn new() -> Self {
        let server = wiremock::MockServer::start().await;
        let base_url = server.uri();
        Self { server, base_url }
    }

    /// Get the base URL of the mock server.
    pub fn uri(&self) -> String {
        self.base_url.clone()
    }

    /// Register a mock response for a method and path.
    ///
    /// Example:
    /// ```ignore
    /// server.mock_response(
    ///     wiremock::matchers::method(wiremock::Method::GET),
    ///     "/api/data",
    ///     200,
    ///     r#"{"key":"value"}"#
    /// ).await;
    /// ```
    pub async fn mock_response<M>(
        &mut self,
        matcher: M,
        path: &str,
        status: u16,
        body: &str,
    ) where
        M: wiremock::matcher::Matcher<wiremock::Request> + Clone + Send + Sync + 'static,
    {
        let response = wiremock::ResponseTemplate::new(status)
            .set_body_string(body);

        wiremock::Mock::given(matcher)
            .and(wiremock::matchers::path(path))
            .respond_with(response)
            .mount(&self.server)
            .await;
    }

    /// Register a mock that returns 429 Rate Limited.
    pub async fn mock_rate_limit(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method(wiremock::Method::GET),
            path,
            429,
            r#"{"error":"Too Many Requests"}"#
        ).await;
    }

    /// Register a mock that returns 500 Server Error.
    pub async fn mock_server_error(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method(wiremock::Method::GET),
            path,
            500,
            r#"{"error":"Internal Server Error"}"#
        ).await;
    }

    /// Register a mock that returns 404 Not Found.
    pub async fn mock_not_found(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method(wiremock::Method::GET),
            path,
            404,
            r#"{"error":"Not Found"}"#
        ).await;
    }
}

/// Load an HTML fixture from the tests/fixtures/ directory.
///
/// # Panics
///
/// Panics if the fixture file cannot be read.
pub fn load_fixture(name: &str) -> String {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    std::fs::read_to_string(&fixture_path).unwrap_or_else(|e| {
        panic!(
            "Failed to load fixture {}: {}",
            fixture_path.display(),
            e
        )
    })
}

/// Get the path to the fixtures directory.
pub fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Create a minimal ScrapedContent for testing.
pub fn mock_scraped_content(url: &str, title: &str, content: &str) -> rust_scraper::ScrapedContent {
    rust_scraper::ScrapedContent {
        title: title.to_string(),
        content: content.to_string(),
        url: rust_scraper::ValidUrl::parse(url).expect("valid test URL"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
    }
}

/// Create a ScrapedContent with raw HTML included.
pub fn mock_scraped_content_with_html(
    url: &str,
    title: &str,
    content: &str,
    html: &str,
) -> rust_scraper::ScrapedContent {
    rust_scraper::ScrapedContent {
        title: title.to_string(),
        content: content.to_string(),
        url: rust_scraper::ValidUrl::parse(url).expect("valid test URL"),
        excerpt: None,
        author: None,
        date: None,
        html: Some(html.to_string()),
        assets: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_fixture_static_page() {
        let html = load_fixture("static_page.html");
        assert!(html.contains("<title>Test Page"));
        assert!(html.contains("Sample Article Title"));
    }

    #[test]
    fn test_fixtures_dir_exists() {
        let dir = fixtures_dir();
        assert!(dir.exists());
        assert!(dir.is_dir());
    }

    #[test]
    fn test_mock_scraped_content() {
        let content = mock_scraped_content(
            "https://example.com/test",
            "Test Title",
            "Test content",
        );
        assert_eq!(content.title, "Test Title");
        assert_eq!(content.content, "Test content");
        assert_eq!(content.url.as_str(), "https://example.com/test");
    }
}
