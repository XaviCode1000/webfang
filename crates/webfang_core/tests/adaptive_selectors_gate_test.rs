//! #1813 — `--adaptive-selectors` must fail closed on a non-adaptive build.
//!
//! The flag used to be a HIDDEN COMPATIBILITY PLACEHOLDER when the
//! `adaptive-selectors` cargo feature was compiled out
//! (`cli/spec_command.rs`), so a non-adaptive binary accepted
//! `--adaptive-selectors`, wired no `AdaptiveSelectorEngine`
//! (`webfang_cli/src/main.rs::build_adaptive_engine` is `#[cfg]`-gated and
//! returns `None` when the flag is off), and exited 0 having done nothing.
//!
//! The gate now lives where every sibling build-capability gate already lives:
//! `preflight::check_adaptive_selectors_feature`, mirroring
//! `check_clean_ai_feature` (#761) → `CliExit::ConfigError` → **exit 78**,
//! before any network request.
//!
//! ## Why the two tests are mutually `cfg`-gated
//!
//! Each lane compiles exactly ONE test, and both lanes are non-empty:
//!
//! - `#[cfg(not(feature = "adaptive-selectors"))]` — the rejection. Runs in
//!   any lane without the feature, e.g.
//!   `cargo nextest run -p webfang_core --features ai,persistence,console
//!   --test adaptive_selectors_gate_test`.
//! - `#[cfg(feature = "adaptive-selectors")]` — the non-regression guard: on a
//!   build WITH the feature the flag must keep working exactly as before.
//!   Runs in `cargo nextest run -p webfang_core --all-features --test
//!   adaptive_selectors_gate_test`.
//!
//! The harness resolves the binary through `webfang_path()`, which rebuilds
//! `webfang_cli` with the test crate's EXACT active feature set
//! (`tests/common/cli_harness.rs`), so the flag under test is really absent
//! from the binary in the negative lane. `cargo nextest list` on each lane is
//! the proof that neither test compiles to nothing.

#[path = "common/cli_harness.rs"]
mod common;

use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use common::cmd;

/// Wall-clock budget for one spawned-binary attempt. Generous because
/// `webfang_path()` may shell out to `cargo build` on a cold target dir.
const RUN_TIMEOUT: Duration = Duration::from_secs(180);

/// The rejection text `preflight::check_adaptive_selectors_feature` emits.
/// A build WITH the feature must never print it — this marker is what makes
/// the positive-lane test a real assertion rather than a weaker "not 78".
const REJECTION_MARKER: &str = "requiere un binario compilado con la feature `adaptive-selectors`";

/// Body served by the mock. Must never be requested in the negative lane: the
/// gate fires in preflight, before the first socket.
const BODY: &str = "<html><body><h1>Hi</h1><p>content long enough to clear the fifty \
                     character minimum content guard comfortably.</p></body></html>";

/// Start the single-page mock fixture and return the server plus its base
/// URL (with the trailing slash the CLI expects).
///
/// Defined without `#[cfg]`: all three tests (both mutually-exclusive lanes)
/// arrange through it, so it is live under every feature combination.
async fn start_mock_site() -> (MockServer, String) {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(BODY))
        .mount(&mock_server)
        .await;
    let base_url = format!("{}/", mock_server.uri());
    (mock_server, base_url)
}

/// Spawn a `--dry-run --max-pages 1` scrape of `base_url`, optionally
/// requesting `--adaptive-selectors`, returning the finished output.
///
/// Each lane compiles exactly one caller (flag-absent in the negative lane,
/// flag-present in the positive lane), so the helper is live under both
/// feature combinations and never trips `dead_code`.
async fn run_dry_run(base_url: &str, with_adaptive_flag: bool) -> std::process::Output {
    let mut command = cmd();
    if with_adaptive_flag {
        command.arg("--adaptive-selectors");
    }
    command
        .arg("--url")
        .arg(base_url)
        .arg("--dry-run")
        .arg("--max-pages")
        .arg("1")
        .timeout(RUN_TIMEOUT)
        .output()
        .expect("spawn webfang")
}

/// Non-adaptive build: `--adaptive-selectors` must exit 78 with a message
/// naming the feature, before any network request (#1813).
#[cfg(not(feature = "adaptive-selectors"))]
#[tokio::test]
async fn adaptive_selectors_on_non_adaptive_build_fails_closed_naming_the_feature() {
    let (mock_server, base_url) = start_mock_site().await;

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--adaptive-selectors")
        .timeout(RUN_TIMEOUT)
        .assert()
        .code(78)
        .stderr(predicates::str::contains("--adaptive-selectors"))
        .stderr(predicates::str::contains("adaptive-selectors"));

    let requests = mock_server.received_requests().await.unwrap_or_default();
    assert!(
        requests.is_empty(),
        "the gate must fail before any network request, got {} request(s)",
        requests.len()
    );
}

/// Non-adaptive build, flag ABSENT: the gate must not fire (#1813). Proves
/// the new check is gated on the request, not on the build alone.
#[cfg(not(feature = "adaptive-selectors"))]
#[tokio::test]
async fn adaptive_gate_is_silent_when_the_flag_is_absent() {
    let (_mock_server, base_url) = start_mock_site().await;

    let output = run_dry_run(&base_url, false).await;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(REJECTION_MARKER),
        "the gate fired without --adaptive-selectors, got: {stderr}"
    );
    assert_ne!(
        output.status.code(),
        Some(78),
        "an ordinary dry-run must not be failed closed by the adaptive gate"
    );
}

/// Adaptive build: `--adaptive-selectors` keeps working exactly as before —
/// the run proceeds past preflight and reaches the seed fetch (#1813
/// non-regression guard; the flag's behavior must not change on a build that
/// HAS the feature).
#[cfg(feature = "adaptive-selectors")]
#[tokio::test]
async fn adaptive_selectors_on_adaptive_build_is_accepted_and_reaches_the_crawl() {
    let (mock_server, base_url) = start_mock_site().await;

    let output = run_dry_run(&base_url, true).await;

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains(REJECTION_MARKER),
        "an adaptive build must never emit the non-adaptive rejection, got: {stderr}"
    );
    assert_ne!(
        output.status.code(),
        Some(78),
        "--adaptive-selectors must not be failed closed on a build that has the \
         feature; stderr: {stderr}"
    );

    let requests = mock_server.received_requests().await.unwrap_or_default();
    assert!(
        !requests.is_empty(),
        "the run must proceed past preflight to the seed fetch"
    );
}
