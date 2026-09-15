//! #1433 RC-4 net: arrival-after-refusal counts for SSRF.
//!
//! Pins that a refused hop costs no further requests, through the REAL
//! `webfang` binary against the shared fixture:
//!
//! | Row | Target                                  | Pinned outcome              |
//! |-----|-----------------------------------------|-------------------------------|
//! | 1   | `/redir-loopback` (302 → 127.0.0.2:9)   | exit 69, exactly 1 arrival  |
//! | 2   | forbidden-literal seed, guard armed     | exit 69, 0 arrivals           |
//!
//! Row 1 runs under the shared `cmd()` posture (entry layer disarmed so the
//! seed reaches the fixture; redirect + resolver layers fully armed): the
//! seed arrival proves the network is live, and the absence of any later
//! arrival proves the redirect guard stopped the hop pre-socket. Row 2 runs
//! in production posture (hatch removed) so the entry guard refuses the
//! seed pre-socket: zero arrivals on a live listener.
//!
//! Child env only — no global mutation, so the rows stay parallel-safe.
//! Determinism: counts only, no timing assertions, no sleeps.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::cmd;
use common::fixture_server::start_fixture;

const RUN_TIMEOUT_SECS: u64 = 120;

/// Collapse whitespace runs so multi-line stderr matches robustly.
fn normalize_captured(text: &str) -> String {
    text.replace('\x1b', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// A redirect to a forbidden literal is stopped after the seed: exactly one
/// arrival (the seed that produced the 302), never the refused hop.
#[tokio::test]
async fn redirect_to_loopback_costs_seed_arrival_only() {
    let (base, log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    let output = cmd()
        .arg("--url")
        .arg(format!("{base}/redir-loopback"))
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(out.path())
        .arg("--timeout-secs")
        .arg("10")
        .arg("--max-retries")
        .arg("0")
        .timeout(std::time::Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");
    let stderr = normalize_captured(&String::from_utf8_lossy(&output.stderr));

    assert_eq!(
        output.status.code(),
        Some(69),
        "a blocked redirect must surface as a typed scrape failure (69): {stderr}"
    );
    assert!(
        stderr.contains("302"),
        "the terminal classification must name the stopped 302 hop: {stderr}"
    );
    assert_eq!(
        log.paths(),
        vec!["/redir-loopback".to_string()],
        "only the seed request may arrive — the refused hop must cost zero requests"
    );
}

/// A forbidden-literal seed is refused at entry, pre-socket: the live
/// listener observes zero arrivals.
#[tokio::test]
async fn refused_literal_seed_costs_zero_arrivals() {
    let (_base, log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    // Production posture: remove the harness's entry-layer hatch so the
    // guard is fully armed. The literal sits on port 9 (discard), so any
    // followed fetch would surface a connect error, never this rejection.
    let output = cmd()
        .env_remove(webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV)
        .arg("--url")
        .arg("http://127.0.0.2:9/")
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(out.path())
        .arg("--timeout-secs")
        .arg("5")
        .arg("--max-retries")
        .arg("0")
        .timeout(std::time::Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");
    let stderr = normalize_captured(&String::from_utf8_lossy(&output.stderr));

    assert_eq!(
        output.status.code(),
        Some(69),
        "a forbidden-literal seed must fail with the typed rejection (69): {stderr}"
    );
    assert!(
        stderr.contains("SSRF detectado") && stderr.contains("127.0.0.2"),
        "the rejection must name the offending IP in Spanish: {stderr}"
    );
    assert_eq!(
        log.count(),
        0,
        "entry refusal happens pre-socket — the listener must observe zero arrivals"
    );
}
