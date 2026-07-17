//! CLI binary integration tests
//!
//! Tests the actual `webfang` binary using `assert_cmd`.
//! These tests verify the binary behaves correctly for edge cases
//! without requiring network access.
//!
//! Run with: cargo nextest run --test-threads 2 cli_binary_test

#[path = "common/cli_harness.rs"]
mod common;
use common::{cmd, redact_nondeterministic};

use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ============================================================================
// Tests: Binary error handling
// ============================================================================

/// Test that running without --url shows an error message
#[test]
fn test_no_url_shows_error() {
    let output = cmd().output().expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_no_url_shows_error",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
}

/// Test that an invalid URL shows an error
#[test]
fn test_invalid_url_shows_error() {
    // CLI validates URL and returns error message
    let output = cmd()
        .arg("--url")
        .arg("not-a-url")
        .output()
        .expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_invalid_url_shows_error",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
}

// ============================================================================
// Tests: Binary help and version
// ============================================================================

/// Test that --help contains scraper description
/// Test that --help prints usage and exits with code 0.
#[test]
fn test_help_contains_scraper() {
    let output = cmd().arg("--help").output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    insta::assert_snapshot!(
        "test_help_contains_scraper",
        redact_nondeterministic(Path::new("__no_temp__"), &stdout)
    );
}

/// Test that --version outputs version and exits with code 0.
#[test]
fn test_version() {
    let output = cmd().arg("--version").output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    insta::assert_snapshot!(
        "test_version",
        redact_nondeterministic(Path::new("__no_temp__"), &stdout)
    );
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
    let output = cmd().arg("--quiet").output().expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_quiet_flag_accepted",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
}

/// Test that --dry-run flag is accepted
#[test]
fn test_dry_run_flag_accepted() {
    let output = cmd().arg("--dry-run").output().expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_dry_run_flag_accepted",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
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

    let output = cmd()
        .arg("--url")
        .arg(format!("{}/slow", mock_server.uri()))
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("1")
        .arg("--output")
        .arg(output_dir.path())
        .arg("--quiet")
        .output()
        .expect("run binary");
    assert_eq!(output.status.code(), Some(69), "expected exit code 69");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_single_page_custom_timeout_is_used_by_scrape_client",
        redact_nondeterministic(output_dir.path(), &stderr)
    );
}
