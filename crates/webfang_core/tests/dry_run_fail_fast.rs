//! Behavioral tests: `--dry-run` fail-fast (change `dry-run-fail-fast`).
//!
//! `--dry-run` is a preview: against a dead seed it MUST report reachability
//! with a single transport attempt and zero backoff sleeps, then exit 69
//! with the existing Spanish `NetworkError` (exit code correct since #1443;
//! this change fixes only the latency). Guard order is untouched: an
//! SSRF-refused seed still exits 2 before any fetch (#1381). The real crawl
//! path keeps full operator retry semantics — pinned by an A/B gate against
//! the frozen pre-fix binary plus one explicit non-default knob test.
//!
//! Run with:
//! `cargo nextest run -p webfang_core --test dry_run_fail_fast`
//!
//! The A/B gate needs the frozen pre-fix binary explicitly:
//! `WEBFANG_FROZEN_BIN=/tmp/opencode/webfang-frozen-dryrun cargo nextest
//! run -p webfang_core --test dry_run_fail_fast`. Without the variable the
//! A/B test skips (early return) — attempt counts are still pinned by the
//! single-attempt and explicit-knob tests, which need no reference binary.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/cli_harness.rs"]
mod common;

use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::{redact_nondeterministic, BehavioralTest};

const RUN_TIMEOUT: Duration = Duration::from_secs(120);
/// Env var carrying the frozen pre-fix binary for the A/B gate.
const FROZEN_BIN_ENV: &str = "WEBFANG_FROZEN_BIN";

// NOTE: poisoned-env stripping (`WEBFANG_*` / `AI_MODEL_ID`) lives in the
// shared harness (`common::strip_poisoned_env`, same floor as `sanitize_env`)
// so the A/B runs stay hermetic even when CI bug-discovery workflows poison
// the environment.

/// Normalize captured output: strip ANSI escape sequences (`ESC[` … final
/// byte, plus any stray `ESC`), then collapse whitespace runs (same floor
/// as `ssrf_rfc1918_e2e_test`, extended so insta snapshots hold no color
/// fragments).
fn normalize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.clone().next() == Some('[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Grab a loopback port and release it: no listener accepts there, so a seed
/// on it is connection-refused deterministically, without any network.
fn closed_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("listener addr").port();
    drop(listener);
    port
}

/// Serve a persistently failing seed: every fetch answers 500 (a retriable
/// 5xx, so the run spends its full retry budget on the seed path).
async fn mount_failing_seed(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(server)
        .await;
}

/// Seed-path attempts in the mock journal. Side traffic is excluded by
/// construction (`--single-page --ignore-robots`), so only seed fetches land
/// on `/`.
async fn seed_attempts(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .filter(|req| req.url.path() == "/")
        .count()
}

/// `--dry-run` against a refused loopback seed: exit 69 with the existing
/// Spanish `NetworkError` and no success listing (contract since #1443;
// kept here as the fail-fast acceptance pin).
#[tokio::test]
async fn dry_run_refused_seed_exits_69_with_spanish_error() {
    // Arrange: refused seed (the harness disarms only the entry guard, so
    // the dial is attempted and refused).
    let port = closed_loopback_port();
    let out = tempfile::TempDir::new().expect("temp output dir");

    // Act.
    let output = common::cmd()
        .arg("--url")
        .arg(format!("http://127.0.0.1:{port}/"))
        .arg("--dry-run")
        .arg("--output")
        .arg(out.path())
        .timeout(RUN_TIMEOUT)
        .output()
        .expect("spawn webfang");

    // Assert.
    let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!output.status.success(), "dead seed must fail: {stderr}");
    assert_eq!(
        output.status.code(),
        Some(69),
        "dead-seed preview keeps exit 69: {stderr}"
    );
    assert!(
        stderr.contains("la semilla no respondió"),
        "existing Spanish NetworkError missing: {stderr}"
    );
    assert!(
        !stdout.contains("would be scraped"),
        "failed preview must print no success listing: {stdout}"
    );
    insta::assert_snapshot!(
        "dry_run_refused_seed_exits_69_with_spanish_error",
        redact_nondeterministic(out.path(), &stderr)
    );
}

/// `--dry-run` against an SSRF-forbidden seed: exit 2 BEFORE any fetch
/// (#1381 guard order, unchanged by fail-fast).
#[tokio::test]
async fn dry_run_ssrf_forbidden_seed_exits_2_before_any_fetch() {
    // Arrange: production posture (re-arm the entry guard the harness
    // disarms) with a tripwire mock behind the forbidden loopback seed.
    let test = BehavioralTest::new().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><h1>Never served</h1></body></html>"),
        )
        .mount(&test.server)
        .await;

    // Act.
    let output = test
        .scraper_cmd()
        .env_remove(webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV)
        .arg("--dry-run")
        .timeout(RUN_TIMEOUT)
        .output()
        .expect("spawn webfang");

    // Assert.
    let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        output.status.code(),
        Some(2),
        "refused seed preview keeps exit 2: {stderr}"
    );
    assert!(
        stderr.contains("cortó la semilla"),
        "refusal message must name the pre-socket cut: {stderr}"
    );
    insta::assert_snapshot!(
        "dry_run_ssrf_forbidden_seed_exits_2_before_any_fetch",
        redact_nondeterministic(test.out.path(), &stderr)
    );
    let requests = test
        .server
        .received_requests()
        .await
        .expect("wiremock request journal");
    assert!(
        requests.is_empty(),
        "guard order violated: {} request(s) opened a socket",
        requests.len()
    );
}

/// `--dry-run` against a persistently failing seed issues exactly ONE seed
/// attempt (fail-fast: `max_retries = 0`, zero backoff sleeps), then exits 69.
#[tokio::test]
async fn dry_run_failing_seed_issues_a_single_attempt() {
    // Arrange.
    let test = BehavioralTest::new().await;
    mount_failing_seed(&test.server).await;

    // Act.
    let output = test
        .scraper_cmd()
        .arg("--dry-run")
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--timeout-secs")
        .arg("5")
        .timeout(RUN_TIMEOUT)
        .output()
        .expect("spawn webfang");

    // Assert.
    let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        output.status.code(),
        Some(69),
        "failing-seed preview keeps exit 69: {stderr}"
    );
    assert_eq!(
        seed_attempts(&test.server).await,
        1,
        "fail-fast preview must attempt the seed exactly once"
    );
}

/// Real crawl at default knobs is behavior-identical to the frozen pre-fix
/// binary (A/B gate): same seed-attempt count, same exit code.
///
/// Needs `WEBFANG_FROZEN_BIN` (frozen pre-fix binary built during apply);
/// skips without it — the single-attempt and explicit-knob tests below still
/// pin the new behavior on their own.
#[tokio::test]
async fn real_crawl_at_defaults_matches_frozen_binary_attempt_counts() {
    let frozen = match std::env::var(FROZEN_BIN_ENV) {
        Ok(path) => path,
        Err(_) => {
            eprintln!("SKIP: {FROZEN_BIN_ENV} unset — A/B gate needs the frozen binary");
            return;
        },
    };
    let current = common::webfang_path();

    let mut observed = Vec::new();
    for (label, bin) in [
        ("frozen", std::path::PathBuf::from(frozen)),
        ("current", current),
    ] {
        // Arrange: fresh failing seed per run (both servers stay alive for
        // the whole test, so the OS never reuses a port and the journals
        // cannot collide).
        let server = MockServer::start().await;
        mount_failing_seed(&server).await;
        let out = tempfile::TempDir::new().expect("temp output dir");

        // Act: identical flags and identical env treatment for both binaries
        // (entry guard disarmed for the loopback mock, poison stripped).
        let mut cmd = assert_cmd::Command::new(&bin);
        common::strip_poisoned_env(&mut cmd);
        let output = cmd
            .env(
                webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
                "1",
            )
            .arg("--url")
            .arg(server.uri())
            .arg("--single-page")
            .arg("--ignore-robots")
            .arg("--timeout-secs")
            .arg("5")
            .arg("--output")
            .arg(out.path())
            .timeout(RUN_TIMEOUT)
            .output()
            .expect("spawn webfang");

        // Assert per run, then compare across runs.
        let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
        observed.push((
            label,
            output.status.code(),
            seed_attempts(&server).await,
            stderr,
        ));
    }

    let [frozen_run, current_run] = observed.as_slice() else {
        panic!("expected exactly two A/B runs");
    };
    assert_eq!(
        current_run.2, frozen_run.2,
        "default-knob attempt counts must match the frozen binary"
    );
    assert_eq!(
        current_run.2, 4,
        "default budget is 1 + 3 retries: frozen={} current={}",
        frozen_run.2, current_run.2
    );
    assert_eq!(
        current_run.1, frozen_run.1,
        "default-knob exit codes must match the frozen binary"
    );
}

/// Real crawl honors an explicit non-default `--max-retries`: 1 retry means
/// exactly 2 seed attempts (operator threading, not built-in defaults).
#[tokio::test]
async fn real_crawl_explicit_max_retries_is_honored() {
    // Arrange.
    let test = BehavioralTest::new().await;
    mount_failing_seed(&test.server).await;

    // Act.
    let output = test
        .scraper_cmd()
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--timeout-secs")
        .arg("5")
        .arg("--max-retries")
        .arg("1")
        .timeout(RUN_TIMEOUT)
        .output()
        .expect("spawn webfang");

    // Assert.
    let stderr = normalize(&String::from_utf8_lossy(&output.stderr));
    assert_eq!(
        seed_attempts(&test.server).await,
        2,
        "explicit --max-retries 1 must yield 1 + 1 attempts: {stderr}"
    );
}
