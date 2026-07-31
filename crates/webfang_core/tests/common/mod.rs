//! Shared test fixtures and helpers for webfang integration tests.
//!
//! Provides reusable test data generators, mock servers, and temporary
//! directory helpers. Consumed by integration tests across the workspace.
//!
//! # Usage
//!
//! ```ignore
//! mod common;
//! use common::{sample_html, sample_sitemap, TestHttpServer};
//! ```

pub mod cli_harness;
pub mod fixtures;
pub mod mock_http;

pub use fixtures::*;

use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[cfg(feature = "persistence")]
use deadpool_sqlite::Pool;

#[allow(dead_code)]
pub struct TestHttpServer {
    server: wiremock::MockServer,
    base_url: String,
}

#[allow(dead_code)]
impl TestHttpServer {
    pub async fn new() -> Self {
        let server = wiremock::MockServer::start().await;
        let base_url = server.uri();
        Self { server, base_url }
    }

    pub fn uri(&self) -> String {
        self.base_url.clone()
    }

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

    pub async fn mock_rate_limit(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method("GET"),
            path,
            429,
            r#"{"error":"Too Many Requests"}"#,
        )
        .await;
    }

    pub async fn mock_server_error(&mut self, path: &str) {
        self.mock_response(
            wiremock::matchers::method("GET"),
            path,
            500,
            r#"{"error":"Internal Server Error"}"#,
        )
        .await;
    }

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

pub fn load_fixture(name: &str) -> String {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures_root")
        .join(name);
    std::fs::read_to_string(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to load fixture {}: {}", fixture_path.display(), e))
}

pub fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures_root")
}

pub fn mock_scraped_content(url: &str, title: &str, content: &str) -> webfang_core::ScrapedContent {
    webfang_core::ScrapedContent {
        title: title.to_string(),
        content: content.to_string(),
        url: webfang_core::ValidUrl::parse(url).expect("valid test URL"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
    }
}

#[allow(dead_code)]
pub fn mock_scraped_content_with_html(
    url: &str,
    title: &str,
    content: &str,
    html: &str,
) -> webfang_core::ScrapedContent {
    webfang_core::ScrapedContent {
        title: title.to_string(),
        content: content.to_string(),
        url: webfang_core::ValidUrl::parse(url).expect("valid test URL"),
        excerpt: None,
        author: None,
        date: None,
        html: Some(html.to_string()),
        assets: Vec::new(),
        correlation_id: None,
    }
}

#[allow(dead_code)]
pub struct MockVault {
    _temp_dir: TempDir,
    vault_path: PathBuf,
}

#[allow(dead_code)]
impl MockVault {
    pub fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let vault_path = temp_dir.path().to_path_buf();

        let obsidian_dir = vault_path.join(".obsidian");
        std::fs::create_dir_all(&obsidian_dir).expect("failed to create .obsidian directory");

        std::fs::write(obsidian_dir.join("workspace.json"), "{}")
            .expect("failed to create workspace.json");

        let vault_fs_path = vault_path.to_string_lossy();
        let obsidian_json = format!(
            r#"{{"vault":{{"fsPath":"{}","id":"test-vault-id","name":"TestVault"}}}}"#,
            vault_fs_path
        );
        std::fs::write(obsidian_dir.join("obsidian.json"), &obsidian_json)
            .expect("failed to create obsidian.json");

        let test_note = "---\ntags: [test]\n---\n# Test Note\n\nMock note content.\n";
        std::fs::write(vault_path.join("test-note.md"), test_note)
            .expect("failed to create test-note.md");

        Self {
            _temp_dir: temp_dir,
            vault_path,
        }
    }

    pub fn path(&self) -> &PathBuf {
        &self.vault_path
    }

    pub fn vault_json(&self) -> PathBuf {
        self.vault_path.join(".obsidian").join("obsidian.json")
    }

    pub fn is_recognized_as_vault(&self) -> bool {
        let obsidian_dir = self.vault_path.join(".obsidian");
        obsidian_dir.is_dir() && obsidian_dir.join("obsidian.json").is_file()
    }
}

#[cfg(feature = "persistence")]
#[allow(dead_code)]
pub struct MemoryDb {
    pool: Pool,
}

#[cfg(feature = "persistence")]
#[allow(dead_code)]
impl MemoryDb {
    pub fn new() -> Self {
        let pool = webfang_core::infrastructure::persistence::create_memory_pool()
            .expect("create_memory_pool must succeed in tests");
        Self { pool }
    }

    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    pub fn into_pool(self) -> Pool {
        self.pool
    }
}
