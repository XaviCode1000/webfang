//! CLI binary integration tests
//!
//! Tests the actual `rust_scraper` binary using `assert_cmd`.
//! These tests verify the binary behaves correctly for edge cases
//! without requiring network access.
//!
//! Run with: cargo nextest run --test-threads 2 cli_binary_test

use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Resolve the path to the `webfang` binary.
///
/// `webfang` is built by the `rust_scraper_cli` crate (a workspace sibling),
/// so `assert_cmd::cargo_bin` cannot locate it from `rust_scraper_core` tests
/// — `CARGO_BIN_EXE_webfang` is only set for the crate that owns the binary.
/// We fall back to the workspace `target/` dir and, if missing, build it.
fn webfang_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return std::path::PathBuf::from(p);
    }
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/rust_scraper_core -> workspace root (two levels up)
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
        .args(["build", "-p", "rust_scraper_cli", "--bin", "webfang", "--quiet"])
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
// Tests: Binary error handling
// ============================================================================

/// Test that running without --url shows an error message
#[test]
fn test_no_url_shows_error() {
    cmd()
        .assert()
        .failure()
        .stderr(predicate::str::contains("--url is required"));
}

/// Test that an invalid URL shows an error
#[test]
fn test_invalid_url_shows_error() {
    // CLI validates URL and returns error message
    cmd()
        .arg("--url")
        .arg("not-a-url")
        .assert()
        .failure() // CLI returns exit code 64
        .stderr(predicate::str::contains("Invalid URL"));
}

// ============================================================================
// Tests: Binary help and version
// ============================================================================

/// Test that --help contains scraper description
/// Test that --help prints usage and exits with code 0.
#[test]
fn test_help_contains_scraper() {
    cmd()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("rust_scraper binary"));
}

/// Test that --version outputs version and exits with code 0.
#[test]
fn test_version() {
    cmd()
        .arg("--version")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

// ============================================================================
// Tests: Dry-run mode
// ============================================================================

/// Test that --dry-run with a valid URL does not fail (but may fail on network)
#[test]
#[ignore = "requires network access"]
fn test_dry_run_with_url() {
    cmd()
        .arg("--url")
        .arg("https://example.com")
        .arg("--dry-run")
        .assert()
        .success();
}

// ============================================================================
// Tests: Feature flags
// ============================================================================

/// Test that --quiet flag is accepted
#[test]
fn test_quiet_flag_accepted() {
    // Should not fail at argument parsing (will fail at network without URL)
    cmd()
        .arg("--quiet")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--url is required"));
}

/// Test that --dry-run flag is accepted
#[test]
fn test_dry_run_flag_accepted() {
    cmd()
        .arg("--dry-run")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--url is required"));
}

// ============================================================================
// Tests: Single-page scraping
// ============================================================================

#[tokio::test]
async fn test_single_page_requests_only_seed_and_writes_output() {
    let mock_server = MockServer::start().await;
    let output_dir = TempDir::new().expect("create temp output dir");
    let seed_html = r#"
        <html>
            <head><title>Single Page Test</title></head>
            <body>
                <main>
                    <article>
                        <h1>Single Page Test</h1>
                        <p>This page has enough meaningful content for the fallback extractor to produce a usable document.</p>
                        <p>The linked page must not be requested while --single-page is active, because discovery is skipped.</p>
                        <a href="/linked">Linked page that should not be fetched</a>
                    </article>
                </main>
            </body>
        </html>
    "#;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(seed_html))
        .expect(1)
        .named("single-page seed request")
        .mount(&mock_server)
        .await;

    cmd()
        .arg("--url")
        .arg(mock_server.uri())
        .arg("--single-page")
        .arg("--output")
        .arg(output_dir.path())
        .arg("--quiet")
        .assert()
        .success();

    let received_requests = mock_server
        .received_requests()
        .await
        .expect("request recording should be enabled");
    assert_eq!(received_requests.len(), 1);
    assert_eq!(received_requests[0].url.path(), "/");

    let output_entries = std::fs::read_dir(output_dir.path())
        .expect("read output dir")
        .count();
    assert!(output_entries > 0, "single-page scrape should write output");
}

#[tokio::test]
async fn test_single_page_custom_timeout_is_used_by_scrape_client() {
    let mock_server = MockServer::start().await;
    let output_dir = TempDir::new().expect("create temp output dir");

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("slow response content")
                .set_delay(Duration::from_secs(2)),
        )
        .expect(1)
        .named("single-page timeout request")
        .mount(&mock_server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/slow", mock_server.uri()))
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("1")
        .arg("--output")
        .arg(output_dir.path())
        .arg("--quiet")
        .assert()
        .code(69)
        .stderr(predicate::str::contains(
            "No pages were successfully scraped",
        ));
}
