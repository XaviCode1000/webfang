//! #1428: `--delay-ms` paces the batch path — server-side arrival proof.
//!
//! Contract: batch runs one engine per URL, and every engine used to mint a
//! fresh [`SharedRateLimiter`](webfang_core::application::rate_limiter::SharedRateLimiter)
//! with a full burst — so each URL started unpaced and cross-URL spacing was
//! structurally impossible. The fix shares ONE run-scoped bucket across the
//! run's engines; these tests prove the pacing is observable at the server.
//!
//! Design (machine-independent):
//!
//! - The default burst is detector-derived (1..=16) and never assumed here —
//!   each test recomputes it with the same
//!   [`BudgetModel`](webfang_core::domain::budget::BudgetModel) call the binary
//!   makes on the same machine, then feeds `burst + 2` URLs. That forces
//!   exactly two refills, so the paced span is `2 * delay_ms` everywhere and
//!   the suite always covers `>= 3` URLs.
//! - Arrival instants are recorded server-side by the shared #1433 fixture
//!   ([`start_fixture`](common::fixture_server::start_fixture) +
//!   [`RequestLog`](common::fixture_server::RequestLog)) — it serves every
//!   `/p*` page with a substantive article body, so no sleeps anywhere: the
//!   test blocks on process exit, then reads the log.
//! - `--ignore-robots` keeps arrivals to exactly one paced fetch per URL, so
//!   the arrival count itself guards against silent skips (a skipped URL would
//!   shrink the span and fake a pass).
//! - Loopback fixture URLs are refused by the production SSRF guards, so the
//!   spawned binary arms ONLY the two documented test-only hatches
//!   (`WEBFANG_DISABLE_SSRF_ENTRY_GUARD` / `WEBFANG_DISABLE_SSRF_RESOLVER`).
//!   Nothing global is mutated: child env only, so the tests stay
//!   parallel-safe under nextest.
//!
//! Migrated onto the shared harness in #1433 (local `ArrivalRecorder`,
//! binary resolver, and env sanitizer replaced by the shared helpers;
//! bodies and assertions equivalent).

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::cmd;
use common::fixture_server::{start_fixture, RequestLog, SPACING_TOLERANCE};
use webfang_core::domain::budget::detector::SystemDetector;
use webfang_core::domain::budget::{BudgetModel, BudgetOverrides};
use webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV;

/// Pacing under test: the paced span is `2 * DELAY_MS`, asserted at
/// [`SPACING_TOLERANCE`] — arrival jitter exists; early refills do not.
const DELAY_MS: u64 = 400;

/// Run `--batch` with `urls` on stdin to completion (blocking — no sleeps).
///
/// Child env only: the shared [`cmd`] sanitizer strips ambient `WEBFANG_*`
/// poison and disarms the entry layer for the loopback fixture; the batch
/// path additionally needs the documented resolver hatch (same posture as
/// the pre-#1433 local helper — what the test proves is unchanged).
fn run_batch(urls: &[String], delay_ms: u64, out: &tempfile::TempDir) {
    let mut batch_cmd = cmd();
    batch_cmd
        .env(DISABLE_VALIDATING_RESOLVER_ENV, "1")
        .arg("--batch")
        .arg("--delay-ms")
        .arg(delay_ms.to_string())
        .arg("--ignore-robots")
        .arg("--output")
        .arg(out.path())
        .write_stdin(urls.join("\n"))
        .timeout(std::time::Duration::from_secs(120))
        .assert()
        .success();
}

/// The detector-derived default burst on THIS machine — the same value the
/// spawned binary derives, so the URL count below forces refills everywhere.
fn default_burst() -> usize {
    BudgetModel::build(BudgetOverrides::default(), &SystemDetector)
        .burst()
        .get() as usize
}

/// `burst + 2` distinct fixture page URLs: the first `burst` consume the
/// immediate permits, the last two force exactly two refills — a paced span
/// of `2 * DELAY_MS` on any machine, with always `>= 3` URLs.
fn paced_urls(base: &str) -> Vec<String> {
    let count = default_burst() + 2;
    assert!(
        count >= 3,
        "the burst tier is non-zero by construction, so burst + 2 covers >= 3 URLs"
    );
    (0..count).map(|i| format!("{base}/p{i}")).collect()
}

/// `(page-arrival count, span in ms)` over the `/p*` page routes.
/// Robots noise (if any) is excluded by the prefix filter.
fn page_arrivals(log: &RequestLog) -> Option<(usize, u128)> {
    let count = log.count_prefix("/p");
    if count == 0 {
        return None;
    }
    Some((count, log.span().as_millis()))
}

/// Positive: `--delay-ms` spaces server-side arrivals across batch URLs at
/// the DEFAULT burst — the pre-#1428 code serves every URL from a fresh full
/// burst, so arrivals land back-to-back and this fails there.
#[tokio::test]
async fn batch_delay_ms_paces_across_urls_at_default_burst() {
    let (base, log) = start_fixture().await;
    let urls = paced_urls(&base);
    let out = tempfile::TempDir::new().expect("temp output dir");
    run_batch(&urls, DELAY_MS, &out);

    let (count, span_ms) = page_arrivals(&log).expect("the paced run must reach the server");
    assert_eq!(
        count,
        urls.len(),
        "every batch URL must arrive exactly once (a skip would shrink the span): {count} != {}",
        urls.len()
    );
    let min_span_ms = (SPACING_TOLERANCE * 2.0 * DELAY_MS as f64) as u128;
    assert!(
        span_ms >= min_span_ms,
        "server-side arrival span {span_ms}ms < {min_span_ms}ms — `--delay-ms {DELAY_MS}` is not pacing across URLs"
    );
}

/// Negative control: `--delay-ms 0` builds no bucket and stays fast — the
/// same URL count must complete far below the paced floor.
#[tokio::test]
async fn batch_delay_ms_zero_stays_fast() {
    let (base, log) = start_fixture().await;
    let urls = paced_urls(&base);
    let out = tempfile::TempDir::new().expect("temp output dir");
    run_batch(&urls, 0, &out);

    let (count, span_ms) = page_arrivals(&log).expect("the unthrottled run must reach the server");
    assert_eq!(
        count,
        urls.len(),
        "every batch URL must arrive exactly once: {count} != {}",
        urls.len()
    );
    let paced_floor_ms = (SPACING_TOLERANCE * 2.0 * DELAY_MS as f64) as u128;
    assert!(
        span_ms < paced_floor_ms,
        "unthrottled span {span_ms}ms must stay far below the paced floor {paced_floor_ms}ms"
    );
}
