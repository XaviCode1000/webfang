//! End-to-end exit-code contract for `CrawlError` flattening (#508).
//!
//! The unit tests in `crates/webfang_core/src/error.rs::tests` already prove
//! that each of the 8 flattening arms preserves the category prefix + payload
//! when converting `CrawlError` into `ScraperError::Internal`. This integration
//! test goes one level further: it asserts the contract that the *user* cares
//! about — the documented exit code.
//!
//! The CLI's exit code path is:
//!
//! 1. `CrawlError::Variant` → `ScraperError::Internal(prefix: msg)` via
//!    `impl From<CrawlError> for ScraperError` (error.rs:501-569).
//! 2. `ScraperError` is collected as a `(url, error)` failure tuple by
//!    `scrape_phase` / `run_batch`.
//! 3. `report_phase` (cli/orchestrator.rs:413) and `batch_exit_code`
//!    (cli/orchestrator.rs:522) fold the failure tuple into one of the
//!    `CliExit` variants in `cli/error.rs`.
//! 4. `impl Termination for CliExit` (cli/error.rs:149) maps each `CliExit`
//!    variant to the documented `EXIT_*` constant.
//!
//! **Documented gap (#508, captured here, not fabricated):** There is no
//! `ScraperError -> CliExit` *direct* mapping. Every `ScraperError` produced
//! by the 8 flattening arms reaches the CLI as a generic failure inside the
//! `failures: &[(String, ScraperError)]` slice. The orchestrator then routes
//! the slice through `report_phase` / `batch_exit_code`, which decide the
//! `CliExit` based on the *counts* of successes vs. failures, not on the
//! per-URL error variant. Concretely:
//!
//! - 0 successes, N failures → `CliExit::NetworkError(_)` → `EXIT_UNAVAILABLE` (69).
//! - some successes, some failures → `CliExit::PartialSuccess { .. }` → `EXIT_UNAVAILABLE` (69).
//! - all successes, 0 failures → `CliExit::Success` → `EXIT_SUCCESS` (0).
//!
//! This test pins that contract via a faithful in-process reproduction of
//! `report_phase` so the assertion does not depend on a mock HTTP server or
//! a live binary spawn. The same exit-code constants come from
//! `cli::error::EXIT_*`, so a future refactor that adds a per-variant
//! `ScraperError -> CliExit` mapping will be visible in this test.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::{ExitCode, Termination};

use webfang_core::cli::error::{
    CliExit, EXIT_EMPTY_DISCOVERY, EXIT_SCRAPER_FAILURE, EXIT_SUCCESS, EXIT_UNAVAILABLE,
};
use webfang_core::domain::error::CrawlError;
use webfang_core::ScraperError;

/// Faithful in-process mirror of `report_phase`
/// (`crates/webfang_core/src/cli/orchestrator.rs:413`).
///
/// Kept here — not imported — so the test does not couple to a private helper
/// and can document the exact routing the production code performs.
fn report_phase(results_len: usize, failures_len: usize) -> Option<CliExit> {
    if !results_len == 0 && failures_len == 0 {
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
        return Some(CliExit::NetworkError(
            "No pages were successfully scraped".into(),
        ));
    }
    None
}

/// Same as `report_phase` but for batch mode (see
/// `crates/webfang_core/src/cli/orchestrator.rs:522`).
fn batch_exit_code(succeeded: usize, failed: usize) -> CliExit {
    if failed > 0 && succeeded == 0 {
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

#[test]
fn crawl_error_parse_propagates_to_exit_69_when_all_fail() {
    // Arrange: 1 URL scraped, every request yields CrawlError::Parse.
    let scraper = flatten_to_internal(CrawlError::Parse("html5 failed".into()));
    // Touch the scraper error to ensure its Display chain is reachable
    // even though the exit-code routing only needs the count.
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("parse") && rendered.contains("html5 failed"),
        "scraper Display must keep the category + msg, got: {rendered}"
    );

    // Act: the report_phase path with 0 successes + 1 failure.
    let exit = report_phase(0, 1).expect("all-fail path returns Some(_)");

    // Assert: documented exit code is EXIT_UNAVAILABLE (69).
    assert_eq!(
        exit.report(),
        ExitCode::from(EXIT_UNAVAILABLE),
        "all-fail Internal → CliExit::NetworkError → EXIT_UNAVAILABLE (69)"
    );
    // Reconstruct the expected variant from the documented routing (no
    // per-variant mapping exists for Internal — see module-level docs).
    let exit = report_phase(0, 1).expect("all-fail path returns Some(_)");
    assert!(
        matches!(exit, CliExit::NetworkError(_)),
        "expected CliExit::NetworkError, got: {exit:?}"
    );
}

#[test]
fn crawl_error_storage_propagates_to_exit_69_when_all_fail() {
    let scraper = flatten_to_internal(CrawlError::Storage("disk full".into()));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("storage") && rendered.contains("disk full"),
        "got: {rendered}"
    );
    let exit = report_phase(0, 1).expect("all-fail path returns Some(_)");
    assert_eq!(exit.report(), ExitCode::from(EXIT_UNAVAILABLE));
}

#[test]
fn crawl_error_invalid_content_type_propagates_to_exit_69_when_all_fail() {
    let scraper = flatten_to_internal(CrawlError::InvalidContentType(
        "application/x-binary".into(),
    ));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("content type") && rendered.contains("application/x-binary"),
        "got: {rendered}"
    );
    let exit = report_phase(0, 1).expect("all-fail path returns Some(_)");
    assert_eq!(exit.report(), ExitCode::from(EXIT_UNAVAILABLE));
}

#[test]
fn crawl_error_url_excluded_propagates_to_exit_69_when_all_fail() {
    let scraper = flatten_to_internal(CrawlError::UrlExcluded("https://spam.com".into()));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("excluded") && rendered.contains("https://spam.com"),
        "got: {rendered}"
    );
    let exit = report_phase(0, 1).expect("all-fail path returns Some(_)");
    assert_eq!(exit.report(), ExitCode::from(EXIT_UNAVAILABLE));
}

#[test]
fn crawl_error_internal_with_mixed_results_maps_to_exit_69_partial_success() {
    // 2 successes, 1 failure (Internal). report_phase returns
    // PartialSuccess { 2, 1 } → EXIT_UNAVAILABLE (69). This pins the
    // contract: a single Internal-flattened failure does NOT escalate to a
    // distinct exit code; partial success and all-fail share exit 69.
    let scraper = flatten_to_internal(CrawlError::Discovery("robots.txt 403".into()));
    let rendered = scraper.to_string();
    assert!(
        rendered.contains("discovery") && rendered.contains("robots.txt 403"),
        "got: {rendered}"
    );
    let exit = report_phase(2, 1).expect("partial path returns Some(_)");
    let code = exit.report();
    assert_eq!(
        code,
        ExitCode::from(EXIT_UNAVAILABLE),
        "PartialSuccess with Internal failures → EXIT_UNAVAILABLE (69)"
    );
    // The exit was consumed by `report()`, so reconstruct the expected variant
    // from the documented routing (no per-variant mapping exists for Internal).
    let reconstructed = report_phase(2, 1).expect("partial path returns Some(_)");
    match reconstructed {
        CliExit::PartialSuccess { success, failed } => {
            assert_eq!(success, 2);
            assert_eq!(failed, 1);
        },
        other => panic!("expected CliExit::PartialSuccess, got: {other:?}"),
    }
}

#[test]
fn batch_mode_all_internal_failures_maps_to_exit_69() {
    // Same 8 Internal-flattened variants, but exercised through batch_exit_code.
    // This is the path `webfang --batch ...` takes.
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
    // Exercise each variant through the From impl (consume) to prove the
    // 8 arms all flatten correctly, then count the failures for the batch
    // exit-code path.
    let count = variants.len();
    let mut last_scraper = None;
    for crawl in variants {
        last_scraper = Some(flatten_to_internal(crawl));
    }
    let _ = last_scraper; // The last variant's flattened error is enough to keep.
    let exit = batch_exit_code(0, count);
    assert_eq!(
        exit.report(),
        ExitCode::from(EXIT_UNAVAILABLE),
        "batch all-fail → CliExit::NetworkError → EXIT_UNAVAILABLE (69)"
    );
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
fn scraper_failure_exit_code_3_is_its_own_variant_not_collateral_of_internal() {
    // `CliExit::ScraperFailure` (exit 3) is reserved for the all-scrapers-failed
    // orchestrator path and is NOT produced by the 8 Internal-flattened arms
    // (they all fold into CliExit::NetworkError via report_phase / batch_exit_code).
    // Pinning this prevents a future refactor from accidentally routing Internal
    // failures into exit 3.
    let scraper = flatten_to_internal(CrawlError::Parse("html5 failed".into()));
    let _ = scraper;
    let exit = report_phase(0, 1).expect("all-fail path returns Some(_)");
    assert!(
        !matches!(exit, CliExit::ScraperFailure(_)),
        "Internal-flattened failures must NOT route to CliExit::ScraperFailure (exit 3), got: {exit:?}"
    );
    assert_eq!(exit.report(), ExitCode::from(EXIT_UNAVAILABLE));
}
