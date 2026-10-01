//! CLI exit-code policy — folds scrape/batch failure sets into a [`CliExit`].
//!
//! Extracted from `cli/orchestrator.rs` as part of the composition-root
//! decomposition (issue #1619, finding F1). This module owns *only* the
//! decision "given these results and failures, which exit do we leave with" —
//! it performs no I/O, no crawling and no scraping.
//!
//! The severity routing itself lives in [`crate::cli::error`] (#839), which
//! holds the canonical class → exit mapping. The functions here are the two
//! call-site adapters (`report_phase` for the single-URL flow, `batch_exit_code`
//! for the batch flow) plus the shared `format_failure` renderer, so both flows
//! keep an identical precedence chain.

use tracing::info;

use crate::cli::error::CliExit;
use crate::domain;

/// Build the user-facing failure line for a single URL.
///
/// At `verbosity` 0 only the top-level `Display` message is shown, which keeps
/// network errors (DNS, connect) to a single readable line. At `verbosity` 1+
/// the full root-cause chain is preserved via `Error::source()` (D4), appending
/// each cause as `← cause`.
pub(crate) fn format_failure(
    url: &str,
    error: &crate::error::ScraperError,
    verbosity: u8,
) -> String {
    let mut chain = error.to_string();
    if verbosity > 0 {
        let mut src = std::error::Error::source(error);
        while let Some(cause) = src {
            chain.push_str(&format!("  ← {cause}"));
            src = cause.source();
        }
    }
    format!("Failed to scrape {url}: {chain}")
}

/// Report failures and determine the exit code.
///
/// Returns `None` if all pages scraped successfully (caller proceeds to export).
/// At `verbosity` 0 only the top-level error message is shown; at 1+ the full
/// root-cause chain is appended (see [`format_failure`]).
///
/// Robots-blocked routing (#705): when NOTHING was scraped and NOTHING failed
/// but `blocked > 0`, every URL was refused by robots.txt — the run exits
/// `CliExit::Forbidden` (77) with a Spanish hint about `--ignore-robots`
/// instead of the misleading "no pages scraped" network error. Any real
/// failure or any scraped page keeps the historical routing below.
///
/// Severity routing (#537 + #706 + matrix rows 21/22) applies when every
/// URL failed:
///
/// 1. An internal-fatal failure wins → `CliExit::ScraperFailure` (3) — an
///    internal bug must not masquerade as a transient network outage.
/// 2. A permanent-kind [`crate::error::ScraperError::Io`] failure is next →
///    `CliExit::IoError` (74) — an unwritable output path is EX_IOERR.
/// 3. Any [`crate::error::ScraperError::ExtractionFailed`] is next →
///    `CliExit::DataFormatError` (65) — the pages were fetched but carried
///    no usable content (JS-only shells, poor fallback).
/// 4. Anything else keeps `CliExit::NetworkError` (69).
///
/// The partial-success case always reports `PartialSuccess` (69) regardless of
/// failure severity: some content was scraped, which is the dominant signal.
pub(crate) fn report_phase(
    results: &[domain::ScrapedContent],
    failures: &[(String, crate::error::ScraperError)],
    blocked: usize,
    verbosity: u8,
) -> Option<CliExit> {
    for (url, error) in failures {
        eprintln!("{}", format_failure(url, error, verbosity));
    }

    if !failures.is_empty() && !results.is_empty() {
        return Some(CliExit::PartialSuccess {
            success: results.len(),
            failed: failures.len(),
        });
    }

    // Canonical robots-blocked override (#705): fires only when NOTHING was
    // scraped and NOTHING failed, delegating to the single implementation in
    // `cli::error` (#839).
    if let Some(exit) =
        crate::cli::error::forbidden_exit_when_all_blocked(results.len(), failures.len(), blocked)
    {
        return Some(exit);
    }

    if results.is_empty() {
        // Exit-code precedence in the all-fail arm: internal-fatal (3)
        // outranks the permanent-Io override (74), which outranks
        // extraction-failed (65), which outranks the transient fallback
        // (69) (#706 + matrix rows 21/22). An internal bug must never
        // masquerade as a data-format error. The Io override sits here —
        // AFTER the InternalFatal sweep, BEFORE extraction-failed — because
        // since `ScraperError::classify` splits by kind, a permanent io
        // error is PermanentFatal (never caught by the 3 sweep), and a run
        // that failed entirely on an unwritable output path must report 74,
        // not fall through to 65/69.
        // Exit-code precedence in the all-fail arm: internal-fatal (3)
        // outranks the permanent-Io override (74), which outranks
        // extraction-failed (65), which outranks the transient fallback
        // (69) (#706 + matrix rows 21/22). An internal bug must never
        // masquerade as a data-format error. The Io override sits here —
        // AFTER the InternalFatal sweep, BEFORE extraction-failed — because
        // since `ScraperError::classify` splits by kind, a permanent io
        // error is PermanentFatal (never caught by the 3 sweep), and a run
        // that failed entirely on an unwritable output path must report 74,
        // not fall through to 65/69. Each arm delegates to its canonical
        // mapping function in `cli::error` (#839) so every exit-code
        // decision has exactly one implementation.
        if let Some(exit) = crate::cli::error::scraper_failure_exit_when_internal_fatal(failures) {
            return Some(exit);
        }
        if let Some(exit) = permanent_io_error_for_failures(failures) {
            return Some(exit);
        }
        if let Some(exit) =
            crate::cli::error::data_format_error_exit_when_extraction_failed(failures)
        {
            return Some(exit);
        }
        eprintln!("No pages were successfully scraped");
        return Some(CliExit::NetworkError(
            "No pages were successfully scraped".into(),
        ));
    }

    info!("Successfully scraped {} pages", results.len());
    None
}

/// Fold an all-failed error set into a severity-aware exit (#537).
///
/// Severity routing lives entirely in the canonical mapping functions of
/// [`crate::cli::error`] (#839): internal-fatal → `ScraperFailure` (3) via
/// `scraper_failure_exit_when_internal_fatal`, permanent-Io → `IoError`
/// (74) via the per-item `permanent_io_error_exit_for`, extraction-failed →
/// `DataFormatError` (65) via `data_format_error_exit_when_extraction_failed`.
/// This local adapter only dispatches the variant through the canonical
/// per-item helper, keeping one implementation of the 74 decision.
fn permanent_io_error_for_failures(
    failures: &[(String, crate::error::ScraperError)],
) -> Option<CliExit> {
    failures.iter().find_map(|(_, e)| match e {
        crate::error::ScraperError::Io(io_err) => {
            crate::cli::error::permanent_io_error_exit_for(io_err)
        },
        _ => None,
    })
}

/// Determine the CLI exit code from batch scrape results.
///
/// Severity routing (#537 + #706 + matrix rows 21/22) for all-fail runs
/// (`failed > 0 && succeeded == 0`):
///
/// 1. An internal-fatal failure wins → `CliExit::ScraperFailure` (3).
/// 2. A permanent-kind I/O failure is next → `CliExit::IoError` (74).
/// 3. Any [`crate::error::ScraperError::ExtractionFailed`] is next →
///    `CliExit::DataFormatError` (65).
/// 4. Anything else keeps `CliExit::NetworkError` (69).
///
/// Partial success remains `CliExit::PartialSuccess` (69) regardless of
/// failure severity — some content was scraped, which is the dominant signal.
pub(crate) fn batch_exit_code(
    succeeded: usize,
    failed: usize,
    errors: &[(String, crate::error::ScraperError)],
) -> CliExit {
    if failed > 0 && succeeded == 0 {
        // Same precedence as `report_phase` (#706): internal-fatal (3)
        // first, then the permanent-Io override (74), then
        // extraction-failed (65), then the transient fallback (69).
        // The 74 override must precede extraction-failed so an all-fail run
        // caused by an unwritable output path reports 74 truthfully. Each
        // arm delegates to its canonical mapping function in `cli::error`
        // (#839) so every exit-code decision has exactly one implementation.
        if let Some(exit) = crate::cli::error::scraper_failure_exit_when_internal_fatal(errors) {
            return exit;
        }
        if let Some(exit) = permanent_io_error_for_failures(errors) {
            return exit;
        }
        if let Some(exit) = crate::cli::error::data_format_error_exit_when_extraction_failed(errors)
        {
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

#[cfg(test)]
mod tests {
    use super::{batch_exit_code, format_failure, report_phase};
    use crate::cli::error::CliExit;

    // ===== format_failure tests =====

    fn network_error() -> crate::error::ScraperError {
        let inner = std::io::Error::other("failed to lookup address information");
        crate::error::ScraperError::Network(Box::new(inner))
    }

    #[test]
    fn format_failure_default_hides_source_chain() {
        let msg = format_failure("https://example.com", &network_error(), 0);

        assert!(
            msg.contains("error de red"),
            "missing top-level message: {msg}"
        );
        assert!(
            !msg.contains('←'),
            "default output must not show the cause chain: {msg}"
        );
    }

    #[test]
    fn format_failure_verbose_shows_source_chain() {
        let msg = format_failure("https://example.com", &network_error(), 1);

        assert!(
            msg.contains('←'),
            "verbose output must show the cause chain: {msg}"
        );
        assert!(msg.contains("failed to lookup address information"));
    }

    // ===== report_phase routing tests (#705) =====

    fn scraped(url: &str) -> crate::domain::ScrapedContent {
        crate::domain::ScrapedContent {
            title: "t".into(),
            content: "c".into(),
            url: crate::domain::ValidUrl::parse(url).expect("valid test url"),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
            quality_hint: None,
        }
    }

    #[test]
    fn report_phase_all_blocked_returns_forbidden() {
        // Nothing scraped, nothing failed, but URLs were blocked by robots.txt:
        // exit 77 with the Spanish hint, not a misleading network error (#705).
        let exit = report_phase(&[], &[], 2, 0);

        match exit {
            Some(CliExit::Forbidden(msg)) => {
                assert!(
                    msg.contains("2 URL(s) bloqueadas por robots.txt"),
                    "missing blocked count: {msg}"
                );
                assert!(
                    msg.contains("--ignore-robots"),
                    "missing --ignore-robots hint: {msg}"
                );
            },
            other => panic!("expected Forbidden, got: {other:?}"),
        }
    }

    #[test]
    fn report_phase_mixed_success_and_blocked_proceeds() {
        // Some pages scraped, the rest blocked: content was produced, so the run
        // proceeds to export exactly as before blocked counting existed.
        let results = vec![scraped("https://example.com/ok")];

        assert!(report_phase(&results, &[], 1, 0).is_none());
    }

    #[test]
    fn report_phase_partial_success_with_blocked_unchanged() {
        // Failures + results dominate over blocked URLs: PartialSuccess (69)
        // semantics are unchanged by the blocked counter.
        let results = vec![scraped("https://example.com/ok")];
        let failures = vec![("https://example.com/bad".to_string(), network_error())];

        let exit = report_phase(&results, &failures, 1, 0);

        assert!(
            matches!(
                exit,
                Some(CliExit::PartialSuccess {
                    success: 1,
                    failed: 1
                })
            ),
            "expected PartialSuccess, got: {exit:?}"
        );
    }

    #[test]
    fn report_phase_all_fail_with_blocked_stays_network_error() {
        // Real failures present: the blocked counter must not mask them — the
        // historical all-fail NetworkError (69) routing wins. A 404 classifies
        // as PermanentFatal (not InternalFatal), so the #537 ScraperFailure
        // arm does not engage.
        let failures = vec![(
            "https://example.com/bad".to_string(),
            crate::error::ScraperError::http(404, "https://example.com/bad"),
        )];

        let exit = report_phase(&[], &failures, 1, 0);

        assert!(
            matches!(exit, Some(CliExit::NetworkError(_))),
            "expected NetworkError, got: {exit:?}"
        );
    }

    #[test]
    fn report_phase_empty_run_without_blocks_stays_network_error() {
        // No results, no failures, no blocks (e.g. zero-URL edge): the
        // historical "No pages were successfully scraped" arm is preserved.
        let exit = report_phase(&[], &[], 0, 0);

        assert!(
            matches!(exit, Some(CliExit::NetworkError(_))),
            "expected NetworkError, got: {exit:?}"
        );
    }

    // ===== batch_exit_code tests =====

    fn internal_err(msg: &str) -> crate::error::ScraperError {
        crate::error::ScraperError::Internal(msg.to_string())
    }

    fn http_err(status: u16, url: &str) -> crate::error::ScraperError {
        crate::error::ScraperError::http(status, url)
    }

    fn extraction_failed(url: &str) -> crate::error::ScraperError {
        crate::error::ScraperError::ExtractionFailed {
            url: url.to_string(),
            reason: "contenido insuficiente (0 caracteres) — la página devolvió muy poco contenido extraíble"
                .to_string(),
        }
    }

    // ===== permanent-Io exit-74 override tests (matrix rows 21/22) =====

    fn io_err(kind: std::io::ErrorKind) -> crate::error::ScraperError {
        crate::error::ScraperError::Io(std::io::Error::new(kind, "io failure"))
    }

    #[test]
    fn batch_exit_code_all_permanent_io_returns_io_error_74() {
        // Matrix row 22: an all-failed run caused by a permanent io error
        // (unwritable output path) must exit 74 (EX_IOERR), not 3 or 69.
        let errors = vec![(
            "https://x.example.com".to_string(),
            io_err(std::io::ErrorKind::PermissionDenied),
        )];

        let exit = batch_exit_code(0, 1, &errors);

        assert!(
            matches!(exit, CliExit::IoError(_)),
            "permanent-kind Io all-fail must route to IoError(74), got {exit:?}"
        );
    }

    #[test]
    fn report_phase_all_permanent_io_returns_io_error_74() {
        // Same contract through the single-page `report_phase` path.
        let failures = vec![(
            "https://x.example.com".to_string(),
            io_err(std::io::ErrorKind::NotFound),
        )];

        let exit = report_phase(&[], &failures, 0, 0);

        assert!(
            matches!(exit, Some(CliExit::IoError(_))),
            "permanent-kind Io all-fail must route to IoError(74), got {exit:?}"
        );
    }

    #[test]
    fn batch_exit_code_transient_io_keeps_network_error_69() {
        // Matrix row 21: transient io kinds keep the class default (69);
        // the 74 override must NOT fire for them.
        let errors = vec![(
            "https://x.example.com".to_string(),
            io_err(std::io::ErrorKind::Interrupted),
        )];

        let exit = batch_exit_code(0, 1, &errors);

        assert!(
            matches!(exit, CliExit::NetworkError(_)),
            "transient-kind Io all-fail must keep NetworkError(69), got {exit:?}"
        );
    }

    #[test]
    fn batch_exit_code_internal_fatal_outranks_permanent_io() {
        // Precedence: the InternalFatal sweep still wins over the Io override —
        // a run with a genuine internal bug reports 3, not 74.
        let errors = vec![
            ("https://x.example.com".to_string(), internal_err("bug")),
            (
                "https://y.example.com".to_string(),
                io_err(std::io::ErrorKind::PermissionDenied),
            ),
        ];

        let exit = batch_exit_code(0, 2, &errors);

        assert!(
            matches!(exit, CliExit::ScraperFailure(_)),
            "InternalFatal must outrank the permanent-Io override, got {exit:?}"
        );
    }

    // ===== extraction-failed exit-65 routing tests (#706) =====

    #[test]
    fn report_phase_all_extraction_failed_returns_data_format_error() {
        // CE-1: a JS-only batch — every failure is the typed ExtractionFailed
        // — must exit 65 (DataFormatError) with the Spanish message.
        let failures = vec![
            (
                "https://js1.example.com".to_string(),
                extraction_failed("https://js1.example.com"),
            ),
            (
                "https://js2.example.com".to_string(),
                extraction_failed("https://js2.example.com"),
            ),
        ];

        let exit = report_phase(&[], &failures, 0, 0);

        match exit {
            Some(CliExit::DataFormatError(msg)) => {
                assert!(
                    msg.contains("extracción sin contenido útil"),
                    "Spanish message expected, got: {msg}"
                );
                assert!(
                    msg.contains("2 URL(s)"),
                    "message must count the failures: {msg}"
                );
            },
            other => panic!("expected DataFormatError, got: {other:?}"),
        }
    }

    #[test]
    fn report_phase_mixed_with_extraction_failed_keeps_partial_success() {
        // CE-2: one success + one ExtractionFailed stays PartialSuccess (69) —
        // extraction failures never outrank a successful scrape.
        let results = vec![scraped("https://ok.example.com")];
        let failures = vec![(
            "https://js.example.com".to_string(),
            extraction_failed("https://js.example.com"),
        )];

        let exit = report_phase(&results, &failures, 0, 0);

        assert!(
            matches!(
                exit,
                Some(CliExit::PartialSuccess {
                    success: 1,
                    failed: 1
                })
            ),
            "expected PartialSuccess, got: {exit:?}"
        );
    }

    #[test]
    fn report_phase_internal_fatal_outranks_extraction_failed() {
        // Precedence: internal-fatal (3) wins over extraction-failed (65) when
        // both failure classes are present in an all-fail run.
        let failures = vec![
            ("https://bug.example.com".to_string(), internal_err("panic")),
            (
                "https://js.example.com".to_string(),
                extraction_failed("https://js.example.com"),
            ),
        ];

        let exit = report_phase(&[], &failures, 0, 0);

        assert!(
            matches!(exit, Some(CliExit::ScraperFailure(_))),
            "internal-fatal must outrank extraction-failed, got: {exit:?}"
        );
    }

    #[test]
    fn batch_exit_code_all_extraction_failed_returns_data_format_error() {
        // CE-3 (batch): poor-fallback all-fails are typed ExtractionFailed too,
        // so an all-failed batch of them exits 65 instead of 69.
        let errors = vec![
            (
                "https://js1.example.com".to_string(),
                extraction_failed("https://js1.example.com"),
            ),
            (
                "https://js2.example.com".to_string(),
                extraction_failed("https://js2.example.com"),
            ),
        ];

        let exit = batch_exit_code(0, 2, &errors);

        match exit {
            CliExit::DataFormatError(msg) => {
                assert!(
                    msg.contains("extracción sin contenido útil"),
                    "Spanish message expected, got: {msg}"
                );
            },
            other => panic!("expected DataFormatError, got: {other:?}"),
        }
    }

    #[test]
    fn batch_exit_code_internal_fatal_outranks_extraction_failed() {
        // Same 3 > 65 precedence inside the batch code path.
        let errors = vec![
            ("https://bug.example.com".to_string(), internal_err("panic")),
            (
                "https://js.example.com".to_string(),
                extraction_failed("https://js.example.com"),
            ),
        ];

        let exit = batch_exit_code(0, 2, &errors);

        assert!(
            matches!(exit, CliExit::ScraperFailure(_)),
            "internal-fatal must outrank extraction-failed, got: {exit:?}"
        );
    }

    #[test]
    fn batch_all_fail_returns_network_error() {
        let errors: Vec<(String, crate::error::ScraperError)> = (0..5)
            .map(|i| {
                (
                    format!("https://x{i}.com"),
                    http_err(404, "https://x{i}.com"),
                )
            })
            .collect();
        let exit = batch_exit_code(0, 5, &errors);
        assert!(
            matches!(exit, CliExit::NetworkError(_)),
            "Expected NetworkError when all URLs failed with non-internal errors, got: {exit:?}"
        );
    }

    #[test]
    fn batch_all_fail_with_internal_fatal_returns_scraper_failure() {
        let errors: Vec<(String, crate::error::ScraperError)> = vec![
            ("https://a.com".to_string(), http_err(404, "https://a.com")),
            ("https://b.com".to_string(), internal_err("bug")),
        ];
        let exit = batch_exit_code(0, 2, &errors);
        assert!(
            matches!(exit, CliExit::ScraperFailure(_)),
            "Expected ScraperFailure when any failure classifies InternalFatal, got: {exit:?}"
        );
    }

    #[test]
    fn batch_all_succeed_returns_success() {
        let exit = batch_exit_code(10, 0, &[]);
        assert!(
            matches!(exit, CliExit::Success),
            "Expected Success when all URLs succeed, got: {exit:?}"
        );
    }

    #[test]
    fn batch_partial_success_returns_partial() {
        let errors: Vec<(String, crate::error::ScraperError)> = (0..2)
            .map(|i| {
                (
                    format!("https://x{i}.com"),
                    http_err(500, "https://x{i}.com"),
                )
            })
            .collect();
        let exit = batch_exit_code(3, 2, &errors);
        match exit {
            CliExit::PartialSuccess { success, failed } => {
                assert_eq!(success, 3, "success count mismatch");
                assert_eq!(failed, 2, "failed count mismatch");
            },
            other => panic!("Expected PartialSuccess, got: {other:?}"),
        }
    }
}
