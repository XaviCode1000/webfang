//! Shared test fixtures and helpers
//!
//! Common utilities for integration tests:
//! - HTML fixture loading
//! - WireMock server setup
//! - Test content generators
//! - MockVault for Obsidian vault testing
//!
//! # Usage
//!
//! ```ignore
//! mod common;
//! use common::{load_fixture, mock_server, MockVault};
//! ```

use std::path::{Path, PathBuf};
use tempfile::TempDir;

use deadpool_sqlite::Pool;

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
    ///     wiremock::matchers::method("GET"),
    ///     "/api/data",
    ///     200,
    ///     r#"{"key":"value"}"#
    /// ).await;
    /// ```
    pub async fn mock_response<M>(&mut self, matcher: M, path: &str, status: u16, body: &str)
    where
        M: wiremock::Match + Send + Sync + 'static,
    {
        let response = wiremock::ResponseTemplate::new(status).set_body_string(body);

        wiremock::Mock::given(matcher)
            .and(wiremock::matchers::path(path))
            .respond_with(response)
            .mount(&self.server)
            .await;
    }

    /// Register a mock that returns 429 Rate Limited.
    pub async fn mock_rate_limit(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method("GET"),
            path,
            429,
            r#"{"error":"Too Many Requests"}"#,
        )
        .await;
    }

    /// Register a mock that returns 500 Server Error.
    pub async fn mock_server_error(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method("GET"),
            path,
            500,
            r#"{"error":"Internal Server Error"}"#,
        )
        .await;
    }

    /// Register a mock that returns 404 Not Found.
    pub async fn mock_not_found(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method("GET"),
            path,
            404,
            r#"{"error":"Not Found"}"#,
        )
        .await;
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
    std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to load fixture {}: {}", fixture_path.display(), e))
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

/// MockVault — RAII test helper that simulates an Obsidian vault environment.
///
/// Creates a temporary directory with the standard Obsidian vault structure:
/// - `.obsidian/` directory
/// - `.obsidian/workspace.json` (empty)
/// - `.obsidian/obsidian.json` (test metadata)
/// - `test-note.md` (sample note with frontmatter)
///
/// The temp directory is automatically cleaned up when the `MockVault` is dropped.
///
/// # Usage
///
/// ```ignore
/// #[test]
/// fn test_vault_structure() {
///     let vault = MockVault::new();
///     assert!(vault.is_recognized_as_vault());
///     // vault.path() gives you the root
///     // vault.vault_json() gives you the obsidian.json path
/// }
/// ```
pub struct MockVault {
    _temp_dir: TempDir,
    vault_path: PathBuf,
}

impl MockVault {
    /// Create a new mock Obsidian vault in a temporary directory.
    ///
    /// Sets up the full `.obsidian/` structure that Obsidian expects,
    /// plus a sample markdown note.
    pub fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let vault_path = temp_dir.path().to_path_buf();

        // Create .obsidian/ directory
        let obsidian_dir = vault_path.join(".obsidian");
        std::fs::create_dir_all(&obsidian_dir).expect("failed to create .obsidian directory");

        // Create empty workspace.json
        std::fs::write(obsidian_dir.join("workspace.json"), "{}")
            .expect("failed to create workspace.json");

        // Create obsidian.json with test metadata
        let vault_fs_path = vault_path.to_string_lossy();
        let obsidian_json = format!(
            r#"{{"vault":{{"fsPath":"{}","id":"test-vault-id","name":"TestVault"}}}}"#,
            vault_fs_path
        );
        std::fs::write(obsidian_dir.join("obsidian.json"), &obsidian_json)
            .expect("failed to create obsidian.json");

        // Create a sample markdown note with frontmatter
        let test_note = "---\ntags: [test]\n---\n# Test Note\n\nMock note content.\n";
        std::fs::write(vault_path.join("test-note.md"), test_note)
            .expect("failed to create test-note.md");

        Self {
            _temp_dir: temp_dir,
            vault_path,
        }
    }

    /// Returns a reference to the vault root path.
    pub fn path(&self) -> &PathBuf {
        &self.vault_path
    }

    /// Returns the path to `.obsidian/obsidian.json`.
    pub fn vault_json(&self) -> PathBuf {
        self.vault_path.join(".obsidian").join("obsidian.json")
    }

    /// Checks if this vault would be recognized as a valid Obsidian vault.
    ///
    /// A vault is recognized when:
    /// - `.obsidian/` directory exists
    /// - `.obsidian/obsidian.json` exists
    pub fn is_recognized_as_vault(&self) -> bool {
        let obsidian_dir = self.vault_path.join(".obsidian");
        obsidian_dir.is_dir() && obsidian_dir.join("obsidian.json").is_file()
    }
}

/// A managed in-memory SQLite database for testing.
///
/// Wraps a [`deadpool_sqlite::Pool`] backed by `:memory:`. The pool **must**
/// stay alive for the entire test lifetime — dropping it closes all
/// connections and the in-memory database is destroyed.
///
/// # Usage
///
/// ```ignore
/// #[tokio::test]
/// async fn test_something_with_sqlite() {
///     let mem = MemoryDb::new();
///     let pool = mem.pool();
///     // ... use pool for testing
/// }
/// ```
pub struct MemoryDb {
    pool: Pool,
}

impl MemoryDb {
    /// Create a new in-memory SQLite database with a single-connection pool.
    pub fn new() -> Self {
        let pool = rust_scraper::infrastructure::persistence::create_memory_pool()
            .expect("create_memory_pool must succeed in tests");
        Self { pool }
    }

    /// Borrow the underlying connection pool.
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Consume the helper and return the pool (for ownership transfer).
    pub fn into_pool(self) -> Pool {
        self.pool
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
        let content =
            mock_scraped_content("https://example.com/test", "Test Title", "Test content");
        assert_eq!(content.title, "Test Title");
        assert_eq!(content.content, "Test content");
        assert_eq!(content.url.as_str(), "https://example.com/test");
    }
}
