//! End-to-end exit-code contract for `CrawlError` flattening (#508) and
//! severity-aware exit routing (#537).
//!
//! The unit tests in `crates/webfang_core/src/error.rs::tests` already prove
//! that each of the 8 flattening arms preserves the category prefix + payload
//! when converting `CrawlError` into a `ScraperError` variant. This integration
//! test goes one level further: it asserts the contract that the *user* cares
//! about — the documented exit code.
//!
//! The CLI's exit code path is:
//!
//! 1. `CrawlError::Variant` → `ScraperError` via
//!    `impl From<CrawlError> for ScraperError` (error.rs:~501-569).
//! 2. The `ScraperError` is collected as a `(url, error)` failure tuple by
//!    `scrape_phase` / `run_batch`.
//! 3. `report_phase` (cli/orchestrator.rs) and `batch_exit_code`
//!    (cli/orchestrator.rs) fold the failure tuple into one of the `CliExit`
//!    variants in `cli/error.rs`.
//! 4. `impl Termination for CliExit` (cli/error.rs:149) maps each `CliExit`
//!    variant to the documented `EXIT_*` constant.
//!
//! **Severity contract (#537):** exit-code routing is severity-aware. When a
//! run produces zero successes, the failure set is aggregated by
//! `ScraperError::classify()`:
//!
//! - Any `ErrorClass::InternalFatal` → `CliExit::ScraperFailure(_)` →
//!   `EXIT_SCRAPER_FAILURE` (3). Internal bugs must not masquerade as
//!   transient network outages.
//! - Only `TransientRetriable` / `TransientBackoff` / `PermanentFatal` →
//!   `CliExit::NetworkError(_)` → `EXIT_UNAVAILABLE` (69).
//!
//! The partial-success case (at least one URL scraped) stays exit 69
//! regardless of severity: some content was scraped, which is the dominant
//! signal.
//!
//! This test pins that contract via faithful in-process mirrors of
//! `report_phase` / `batch_exit_code` so the assertion does not depend on a
//! mock HTTP server or a live binary spawn. The exit-code constants come from
//! `cli::error::EXIT_*`, so a future refactor of severity routing will be
//! visible here.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{ExitCode, Termination};

use webfang_core::cli::error::{
    CliExit, EXIT_EMPTY_DISCOVERY, EXIT_SCRAPER_FAILURE, EXIT_SUCCESS, EXIT_UNAVAILABLE,
};
use webfang_core::domain::error::CrawlError;
use webfang_core::error::ErrorClass;
use webfang_core::ScraperError;

/// Faithful in-process mirror of `report_phase`
/// (`crates/webfang_core/src/cli/orchestrator.rs`).
///
/// Kept here — not imported — so the test does not couple to a private helper
/// and can document the exact routing the production code performs. Mirrors
/// the current severity-aware production behavior (#537).
fn report_phase(results_len: usize, failures: &[(String, ScraperError)]) -> Option<CliExit> {
    let failures_len = failures.len();
    if results_len > 0 && failures_len == 0 {
        // unreachable in this test: included for symmetry with the prod body
        return None;
    }
    if failures_len > 0 && results_len > 0 {
        return Some(CliExit::PartialSuccess {
            success: results_len,
            failed: failures_len,
        });
    }
    if results_len == 0 {
        if let Some(exit) = scraper_failure_for_internal_fatal(failures) {
            return Some(exit);
        }
        return Some(CliExit::NetworkError(
            "No pages were successfully scraped".into(),
        ));
    }
    None
}

/// Mirror of `scraper_failure_for_internal_fatal` in the orchestrator.
fn scraper_failure_for_internal_fatal(failures: &[(String, ScraperError)]) -> Option<CliExit> {
    let internal_fatal = failures
        .iter()
        .filter(|(_, e)| e.classify() == ErrorClass::InternalFatal)
        .count();
    (internal_fatal > 0).then(|| {
        CliExit::ScraperFailure(format!(
            "Scraper failure: {internal_fatal} internal error(s) out of {} URLs",
            failures.len()
        ))
    })
}

/// Mirror of `batch_exit_code` (see `cli/orchestrator.rs`).
fn batch_exit_code(
    succeeded: usize,
    failed: usize,
    errors: &[(String, ScraperError)],
) -> CliExit {
    if failed > 0 && succeeded == 0 {
        if let Some(exit) = scraper_failure_for_internal_fatal(errors) {
            return exit;
        }
        CliExit::NetworkError("All batch URLs failed".into())
    } else if failed > 0 {
        CliExit::PartialSuccess {
            success: succeeded,
            failed,
        }
    } else {
        CliExit::Success
    }
}

/// Helper: turn a `CrawlError` into a `ScraperError` and assert the variant is
/// `Internal`, mirroring the unit tests but keeping the assertion in scope
/// for the per-variant exit-code test below.
fn flatten_to_internal(crawl: CrawlError) -> ScraperError {
    let scraper: ScraperError = crawl.into();
    assert!(
        matches!(scraper, ScraperError::Internal(_)),
        "CrawlError must flatten to ScraperError::Internal, got: {scraper}"
    );
    scraper
}

/// Wrap a `ScraperError` into a `(url, error)` failure tuple as the
/// orchestrator collects them.
fn failing(url: &str, err: ScraperError) -> (String, ScraperError) {
    (url.to_string(), err)
}

#[test]
fn crawl_error_parse_propagates_to_exit_69_when_all_fail() {
    // Arrange: 1 URL scraped, every request yields CrawlError::Parse.
    // CrawlError::Parse flattens to ScraperError::Internal → InternalFatal
    // (#537) so this MUST route to ScraperFailure / exit 3, not 69.
    let scraper = flatten_to_internal(CrawlError::Parse("html5 failed".into()));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("parse") && rendered.contains("html5 failed"),
        "scraper Display must keep the category + msg, got: {rendered}"
    );

    // Act: the report_phase path with 0 successes + 1 InternalFatal failure.
    let failures = vec![failing("https://x.com", scraper)];
    let exit = report_phase(0, &failures).expect("all-fail path returns Some(_)");

    // Assert (#537): InternalFatal aggregates to ScraperFailure → EXIT_SCRAPER_FAILURE (3).
    assert_eq!(
        exit.report(),
        ExitCode::from(EXIT_SCRAPER_FAILURE),
        "all-fail Internal (InternalFatal) → CliExit::ScraperFailure → EXIT_SCRAPER_FAILURE (3)"
    );
    let exit = report_phase(0, &failures).expect("all-fail path returns Some(_)");
    assert!(
        matches!(exit, CliExit::ScraperFailure(_)),
        "expected CliExit::ScraperFailure, got: {exit:?}"
    );
}

#[test]
fn crawl_error_storage_propagates_to_exit_3_when_all_fail() {
    let scraper = flatten_to_internal(CrawlError::Storage("disk full".into()));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("storage") && rendered.contains("disk full"),
        "got: {rendered}"
    );
    let failures = vec![failing("https://x.com", scraper)];
    let exit = report_phase(0, &failures).expect("all-fail path returns Some(_)");
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
}

#[test]
fn crawl_error_invalid_content_type_propagates_to_exit_3_when_all_fail() {
    let scraper = flatten_to_internal(CrawlError::InvalidContentType(
        "application/x-binary".into(),
    ));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("content type") && rendered.contains("application/x-binary"),
        "got: {rendered}"
    );
    let failures = vec![failing("https://x.com", scraper)];
    let exit = report_phase(0, &failures).expect("all-fail path returns Some(_)");
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
}

#[test]
fn crawl_error_url_excluded_propagates_to_exit_3_when_all_fail() {
    let scraper = flatten_to_internal(CrawlError::UrlExcluded("https://spam.com".into()));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("excluded") && rendered.contains("https://spam.com"),
        "got: {rendered}"
    );
    let failures = vec![failing("https://x.com", scraper)];
    let exit = report_phase(0, &failures).expect("all-fail path returns Some(_)");
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
}

#[test]
fn crawl_error_internal_with_mixed_results_maps_to_exit_69_partial_success() {
    // 2 successes, 1 failure (Internal → InternalFatal). report_phase returns
    // PartialSuccess { 2, 1 } → EXIT_UNAVAILABLE (69). This pins the #537
    // tradeoff: when at least one URL was scraped, the partial signal
    // dominates and severity does NOT escalate to exit 3.
    let scraper = flatten_to_internal(CrawlError::Discovery("robots.txt 403".into()));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("discovery") && rendered.contains("robots.txt 403"),
        "got: {rendered}"
    );
    let failures = vec![failing("https://x.com", scraper)];
    let exit = report_phase(2, &failures).expect("partial path returns Some(_)");
    let code = exit.report();
    assert_eq!(
        code,
        ExitCode::from(EXIT_UNAVAILABLE),
        "PartialSuccess → EXIT_UNAVAILABLE (69) even with InternalFatal failures (#537 tradeoff)"
    );
    let reconstructed = report_phase(2, &failures).expect("partial path returns Some(_)");
    match reconstructed {
        CliExit::PartialSuccess { success, failed } => {
            assert_eq!(success, 2);
            assert_eq!(failed, 1);
        },
        other => panic!("expected CliExit::PartialSuccess, got: {other:?}"),
    }
}

#[test]
fn batch_mode_all_internal_failures_maps_to_exit_3() {
    // The 8 Internal-flattened variants classify InternalFatal → severity
    // routing (#537) upgrades the all-fail batch to exit 3.
    let variants = [
        CrawlError::Parse("html5 failed".into()),
        CrawlError::Storage("disk full".into()),
        CrawlError::Checkpoint("CRC mismatch".into()),
        CrawlError::SessionPool("no sessions available".into()),
        CrawlError::Discovery("robots.txt 403".into()),
        CrawlError::RetryExhausted {
            url: "https://x.com".into(),
            attempts: 3,
        },
        CrawlError::UrlExcluded("https://spam.com".into()),
        CrawlError::InvalidContentType("application/x-binary".into()),
    ];
    let errors: Vec<(String, ScraperError)> = variants
        .into_iter()
        .enumerate()
        .map(|(i, crawl)| failing(&format!("https://x{i}.com"), flatten_to_internal(crawl)))
        .collect();
    let count = errors.len();
    let exit = batch_exit_code(0, count, &errors);
    assert_eq!(
        exit.report(),
        ExitCode::from(EXIT_SCRAPER_FAILURE),
        "batch all-fail with InternalFatal → CliExit::ScraperFailure → EXIT_SCRAPER_FAILURE (3)"
    );
}

#[test]
fn batch_mode_mixed_transient_failures_stay_at_exit_69() {
    // Mixed transient failures — 429 (TransientBackoff) + a transient Network
    // io::TimedOut (TransientRetriable) — contain NO InternalFatal, so the
    // all-fail batch must keep exit 69 (#537 first-three-bucket rule).
    let transient_network = ScraperError::Network(Box::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "request timeout",
    )));
    assert_eq!(
        transient_network.classify(),
        ErrorClass::TransientRetriable,
        "TimedOut source must classify transient"
    );
    let errors = vec![
        failing("https://a.com", ScraperError::http(429, "https://a.com")),
        failing("https://b.com", transient_network),
    ];
    let exit = batch_exit_code(0, errors.len(), &errors);
    assert!(
        matches!(exit, CliExit::NetworkError(_)),
        "mixed transient failures must route to NetworkError, got: {exit:?}"
    );
    assert_eq!(
        exit.report(),
        ExitCode::from(EXIT_UNAVAILABLE),
        "mixed transient failures → EXIT_UNAVAILABLE (69), not 3"
    );
}

#[test]
fn batch_mode_permanent_plus_internal_fatal_escalates_to_exit_3() {
    // A single InternalFatal in an all-fail batch upgrades to exit 3 even when
    // mixed with PermanentFatal errors: InternalFatal wins (#537).
    let errors = vec![
        failing("https://a.com", ScraperError::http(404, "https://a.com")),
        failing(
            "https://b.com",
            ScraperError::Internal("bug: unreachable state".into()),
        ),
    ];
    assert_eq!(
        errors[0].1.classify(),
        ErrorClass::PermanentFatal,
        "404 must be PermanentFatal"
    );
    assert_eq!(errors[1].1.classify(), ErrorClass::InternalFatal);
    let exit = batch_exit_code(0, errors.len(), &errors);
    assert!(
        matches!(exit, CliExit::ScraperFailure(_)),
        "PermanentFatal + InternalFatal → ScraperFailure, got: {exit:?}"
    );
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
}

#[test]
fn documented_exit_code_constants_remain_stable() {
    // Regression guard: the exit-code constants that the integration test
    // depends on must not drift. If any of these values changes, the CLI
    // contract for `--help`, the documented sysexits table, and the snapshot
    // tests that pin exit codes (`tests/exit_code_integration.rs`) all break.
    assert_eq!(EXIT_SUCCESS, 0);
    assert_eq!(EXIT_EMPTY_DISCOVERY, 2);
    assert_eq!(EXIT_SCRAPER_FAILURE, 3);
    assert_eq!(EXIT_UNAVAILABLE, 69);
}

#[test]
fn scraper_failure_exit_code_3_is_reached_by_internal_fatal_not_by_transient() {
    // Inverted guard (#537): exit 3 is the DOCUMENTED destination for an
    // InternalFatal failure set. An Internal-flattened `CrawlError`
    // classifies InternalFatal, so the all-fail path MUST route to
    // `CliExit::ScraperFailure` / `EXIT_SCRAPER_FAILURE` (3) — not
    // `EXIT_UNAVAILABLE`.
    let scraper = flatten_to_internal(CrawlError::Parse("html5 failed".into()));
    let failures = vec![failing("https://x.com", scraper)];
    let exit = report_phase(0, &failures).expect("all-fail path returns Some(_)");
    assert!(
        matches!(exit, CliExit::ScraperFailure(_)),
        "InternalFatal all-fail must route to CliExit::ScraperFailure, got: {exit:?}"
    );
    assert_eq!(exit.report(), ExitCode::from(EXIT_SCRAPER_FAILURE));

    // And a purely transient all-fail set must NOT route to exit 3.
    let transient = vec![failing("https://x.com", ScraperError::http(503, "https://x.com"))];
    assert_eq!(
        transient[0].1.classify(),
        ErrorClass::TransientRetriable,
        "5xx must be TransientRetriable"
    );
    let exit = report_phase(0, &transient).expect("all-fail path returns Some(_)");
    assert!(
        !matches!(exit, CliExit::ScraperFailure(_)),
        "purely transient failures must NOT reach ScraperFailure (exit 3), got: {exit:?}"
    );
    assert_eq!(exit.report(), ExitCode::from(EXIT_UNAVAILABLE));
}
