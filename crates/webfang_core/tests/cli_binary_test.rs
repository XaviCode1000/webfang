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
/// Only runs when both the `ai` and `adaptive-selectors` features are enabled,
/// because the snapshot includes AI-specific flags (present only with
/// `--features ai`) and the `--adaptive-selectors` flag (present only with
/// `--features adaptive-selectors`).
#[cfg(all(feature = "ai", feature = "adaptive-selectors"))]
#[test]
fn test_help_contains_scraper() {
    let output = cmd().arg("--help").output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"(?m)^Parsed using .+$", "Parsed using [REDACTED]");
    settings.bind(|| {
        insta::assert_snapshot!(
            "test_help_contains_scraper",
            redact_nondeterministic(Path::new("__no_temp__"), &stdout)
        );
    });
}

/// Test that --version outputs the workspace version and exits with code 0.
///
/// Deliberately NOT an insta snapshot: the version string changes by design on
/// every release-plz bump, so a golden file here fails every Release PR that
/// does not also remember to rewrite it (webfang#1543). The expected value is
/// derived from the same `CARGO_PKG_VERSION` the binary embeds — assert the
/// contract, not a frozen copy of it.
#[test]
fn test_version() {
    let output = cmd().arg("--version").output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        format!("webfang {}", env!("CARGO_PKG_VERSION")),
        "`webfang --version` must print the current package version"
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
// Tests: doctor / show help text (false-provenance guard)
// ============================================================================

/// `webfang --help` must NOT promise that `show` dumps what a run will use:
/// ADR-0004 wires `[[model_profiles]]` into the cleaner in Tramo D, so the
/// help text describes a configured budget, not a consumed one. The snapshot
/// in `test_help_contains_scraper` baked in the old, false wording; this
/// asserts the corrected contract directly against the binary.
///
/// Issue #1933 (ADR-0004 step 4).
#[test]
fn test_issue_1933_show_about_does_not_claim_the_run_uses_the_profile() {
    let output = cmd().args(["show", "--help"]).output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stdout.contains("the run will use"),
        "show's help must not claim the run uses the configured profile \
         (Tramo D is not wired), got:\n{stdout}"
    );
    assert!(
        stdout.contains("configured profile and budget"),
        "show's help must describe the CONFIGURED budget, got:\n{stdout}"
    );
}

/// `webfang doctor` must disclose the Tramo D boundary in its budget block:
/// the profile numbers are CONFIGURED, not consumed by a run yet. Captured
/// from the real binary so the assertion is on what the operator actually
/// sees, not on an in-source string mirrored by the test.
///
/// Issue #1933 (ADR-0004 step 4).
#[test]
fn test_issue_1933_doctor_budget_discloses_the_tramo_d_boundary() {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "[[providers]]\nid = \"known\"\ndisplay_name = \"Known\"\n\
         kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
         auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
         model = \"nomic-embed-text\"\n",
    )
    .expect("write config");

    let output = cmd()
        .arg("doctor")
        .env("WEBFANG_CONFIG", &config)
        .output()
        .expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);

    // `doctor` may exit non-zero if the reachability probe fails (no network
    // in CI); the budget block prints regardless. Assert on the text, not the
    // exit code.
    assert!(
        stdout.contains("Tramo D"),
        "doctor's budget block must name the Tramo D boundary, got:\n{stdout}"
    );
    assert!(
        stdout.contains("no está cableado al run"),
        "doctor must say the profile is not wired to the run yet, got:\n{stdout}"
    );
}

/// `webfang --embedding-provider <local-onnx-id> doctor` must NOT hard-fail:
/// `llm_wire::build_embedding_provider` maps `ProviderKind::LocalOnnx` to
/// `Ok(None)` precisely to mean "the local ONNX pool serves this slot" — the
/// same provider a run uses happily. Reporting a `ConfigError` there was a
/// false diagnostic an operator would act on.
///
/// Deliberately NOT an insta snapshot and NOT feature-gated: the contract is
/// the exit code plus the Spanish sentence that explains what the local pool
/// means, both of which are stable and worth asserting directly.
#[test]
fn test_doctor_reports_a_local_onnx_provider_as_served_by_the_local_pool() {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("config.toml");
    std::fs::write(
        &config,
        "[[providers]]\nid = \"local\"\ndisplay_name = \"Local\"\n\
         kind = \"local_onnx\"\nbase_url = \"http://127.0.0.1:8080/\"\n\
         auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n",
    )
    .expect("write config");

    let output = cmd()
        .arg("--embedding-provider")
        .arg("local")
        .arg("doctor")
        .env("WEBFANG_CONFIG", &config)
        .output()
        .expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        output.status.code(),
        Some(0),
        "a local-ONNX provider serves the slot; doctor must exit 0, got:\n{stdout}"
    );
    assert!(
        stdout.contains("pool local"),
        "doctor must say the local ONNX pool serves this provider, got:\n{stdout}"
    );
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
    // Filter out infrastructure requests (robots.txt) — we only care about
    // page fetches. The robots_utils fix now correctly fetches robots.txt
    // from the mock server origin (previously it hardcoded https:// and failed).
    let page_requests: Vec<_> = received_requests
        .iter()
        .filter(|r| r.url.path() != "/robots.txt")
        .collect();
    assert_eq!(page_requests.len(), 1);
    assert_eq!(page_requests[0].url.path(), "/");

    let output_entries = std::fs::read_dir(output_dir.path())
        .expect("read output dir")
        .count();
    assert!(output_entries > 0, "single-page scrape should write output");
}

#[tokio::test]
async fn test_single_page_custom_timeout_is_used_by_scrape_client() {
    let mock_server = MockServer::start().await;
    let output_dir = TempDir::new().expect("create temp output dir");

    // Per-request timeout is the configured ceiling for EACH attempt.
    // FIX-1 F-08 (#1231): timeouts are retriable, so the fetch runs 1
    // initial attempt + `max_retries` (default 3) retries — each attempt
    // still capped by the custom 1s timeout — then exits 69 with the
    // last timeout. The 3 retry WARN lines in the stderr snapshot are
    // the observability of that budget. (Mid-body transients also retry:
    // #649.)
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("slow response content")
                .set_delay(Duration::from_secs(2)),
        )
        .expect(4) // 1 attempt + 3 retries (default max_retries)
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
    // The custom timeout value is this test's whole premise, and the exact
    // value is the one thing `redact_nondeterministic` can no longer show:
    // #1631 collapses the network-failure surface to a single `<NET_ERR>` token,
    // because an unreachable host refuses on Unix but times out on Windows and
    // one snapshot cannot hold both. What survives without this line: exit 69
    // plus the 4-request `expect` above prove a timeout under the mock's 2s
    // delay fired and was retried 3 times. What it cannot prove is WHICH value
    // — `timeout_secs` is a `u64`, so 0 and 1 both time out at 0s and 1s and
    // both yield exit 69. Only the message separates them, so pin it here on
    // the un-redacted stderr. Stable across platforms by construction:
    // `request timed out after {0}s` is webfang's own `DownloadError::Timeout`
    // variant (`domain/downloader_port.rs:99`), not a std or OS string.
    assert!(
        stderr.contains("request timed out after 1s"),
        "the --timeout-secs 1 value must reach the scrape client; stderr was: {stderr}"
    );
    insta::assert_snapshot!(
        "test_single_page_custom_timeout_is_used_by_scrape_client",
        redact_nondeterministic(output_dir.path(), &stderr)
    );
}
