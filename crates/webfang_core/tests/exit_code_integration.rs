//! Exit code integration tests
//!
//! Verifies that the CLI returns correct exit codes for:
//! - Empty sitemap discovery → exit 2 (EXIT_EMPTY_DISCOVERY)
//! - Network timeout → exit 69 (EXIT_UNAVAILABLE)
//! - Successful crawl → exit 0 (EXIT_SUCCESS)
//!
//! Run with: cargo nextest run --test-threads 2 exit_code_integration

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Resolve the path to the `webfang` binary.
///
/// `webfang` is built by `webfang_cli` (a workspace sibling), so
/// `assert_cmd::cargo_bin` cannot resolve it — `CARGO_BIN_EXE_webfang`
/// is only set for the owning crate.  This fallback searches
/// `target/{debug,release}` and builds the binary on demand.
fn webfang_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return PathBuf::from(p);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve workspace root");
    for profile in ["debug", "release"] {
        let mut candidate = workspace_root.join("target").join(profile).join("webfang");
        if cfg!(windows) {
            candidate.set_extension("exe");
        }
        if candidate.exists() {
            return candidate;
        }
    }
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"])
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    let mut built = workspace_root.join("target").join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
}

fn cmd() -> Command {
    Command::new(webfang_path())
}

// ============================================================================
// Tests: Empty sitemap → exit 2
// ============================================================================

/// Empty sitemap (no <loc> entries) returns exit code 2.
#[tokio::test]
async fn test_empty_sitemap_returns_exit_2() {
    let mock_server = MockServer::start().await;

    // Serve an empty sitemap
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#,
        ))
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let sitemap_url = format!("{}/sitemap.xml", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("No URLs discovered"));
}

// ============================================================================
// Tests: Network timeout → exit 69
// ============================================================================

/// Timeout during sitemap fetch returns exit code 69.
#[tokio::test]
async fn test_timeout_returns_exit_69() {
    let mock_server = MockServer::start().await;

    // Serve a response with a very long delay to trigger timeout
    Mock::given(method("GET"))
        .and(path("/slow-sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://PLACEHOLDER/page1</loc></url>
</urlset>"#,
                )
                .set_delay(Duration::from_secs(120)),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let sitemap_url = format!("{}/slow-sitemap.xml", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--timeout-secs")
        .arg("1")
        .timeout(Duration::from_secs(60))
        .assert()
        .code(69)
        .stderr(predicate::str::contains("URL discovery failed"));
}

// ============================================================================
// Tests: Successful discovery → exit 0
// ============================================================================

/// Valid sitemap with URLs returns exit code 0 (no regression).
#[tokio::test]
async fn test_valid_sitemap_returns_exit_0() {
    let mock_server = MockServer::start().await;
    let server_uri = mock_server.uri();

    // Serve a valid sitemap with one URL
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>{server_uri}/page1</loc></url>
</urlset>"#
        )))
        .mount(&mock_server)
        .await;

    // Serve the page content
    Mock::given(method("GET"))
        .and(path("/page1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "<html><body><h1>Hello World</h1><p>Test content</p></body></html>",
            ),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", server_uri);
    let sitemap_url = format!("{}/sitemap.xml", server_uri);

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(0);
}
