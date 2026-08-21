//! CLI Error Types and Exit Codes
//!
//! T-050: CliError enum with thiserror
//! T-051: CliExit enum with Termination trait for sysexits codes

use std::process::ExitCode;
use thiserror::Error;

use crate::error::{ErrorClass, ScraperError};

// ============================================================================
// Exit code constants
// ============================================================================

/// Exit 0 — Operation completed successfully.
pub const EXIT_SUCCESS: u8 = 0;
/// Exit 2 — Technical success, no URLs found (empty discovery).
pub const EXIT_EMPTY_DISCOVERY: u8 = 2;
/// Exit 3 — All scrapers failed on discovered URLs.
pub const EXIT_SCRAPER_FAILURE: u8 = 3;
/// Exit 64 — Bad CLI arguments (sysexits EX_USAGE).
pub const EXIT_USAGE_ERROR: u8 = 64;
/// Exit 65 — Data format error (sysexits EX_DATAERR).
pub const EXIT_DATA_ERROR: u8 = 65;
/// Exit 69 — Infrastructure/network failure (sysexits EX_UNAVAILABLE).
pub const EXIT_UNAVAILABLE: u8 = 69;
/// Exit 74 — File I/O error (sysexits EX_IOERR).
pub const EXIT_IO_ERROR: u8 = 74;
/// Exit 76 — Protocol error (sysexits EX_PROTOCOL).
pub const EXIT_PROTOCOL: u8 = 76;
/// Exit 77 — All URLs blocked by robots.txt (sysexits EX_NOPERM).
pub const EXIT_FORBIDDEN: u8 = 77;
/// Exit 78 — Configuration error (sysexits EX_CONFIG).
pub const EXIT_CONFIG: u8 = 78;

// ============================================================================
// T-050: CliError enum
// ============================================================================

/// Categorized CLI errors with user-friendly suggestions.
#[derive(Error, Debug)]
pub enum CliError {
    /// Configuration file error (invalid TOML, missing fields, etc.)
    #[error("Configuración: {msg}\n  Sugerencia: {suggestion}")]
    ConfigFile {
        /// Error message describing the issue
        msg: String,
        /// Suggested fix or workaround
        suggestion: String,
    },

    /// Network-related error (DNS, connection, timeout)
    #[error("Red: {msg}\n  Sugerencia: {suggestion}")]
    NetworkError {
        /// Error message describing the network failure
        msg: String,
        /// Suggested fix or workaround
        suggestion: String,
    },

    /// Some URLs succeeded, others failed
    #[error("Éxito parcial: {success} exitosos, {failed} fallidos\n  Sugerencia: {suggestion}")]
    PartialSuccess {
        /// Number of successfully scraped URLs
        success: u32,
        /// Number of failed URLs
        failed: u32,
        /// Suggested action
        suggestion: String,
    },

    /// Pre-flight validation failed (e.g., unreachable seed URL)
    #[error("Verificación previa fallida: {msg}\n  Sugerencia: {suggestion}")]
    PreflightFailed {
        /// Error message describing the failure
        msg: String,
        /// Suggested fix or workaround
        suggestion: String,
    },
}

impl CliError {
    /// Get the human-readable category name for this error.
    pub fn category(&self) -> &'static str {
        match self {
            CliError::ConfigFile { .. } => "Configuración",
            CliError::NetworkError { .. } => "Red",
            CliError::PartialSuccess { .. } => "Éxito parcial",
            CliError::PreflightFailed { .. } => "Verificación previa",
        }
    }

    /// Get the suggestion text for this error.
    pub fn suggestion(&self) -> &str {
        match self {
            CliError::ConfigFile { suggestion, .. } => suggestion,
            CliError::NetworkError { suggestion, .. } => suggestion,
            CliError::PartialSuccess { suggestion, .. } => suggestion,
            CliError::PreflightFailed { suggestion, .. } => suggestion,
        }
    }
}

/// Format a CliError for display, respecting NO_COLOR setting.
pub fn format_cli_error(err: &CliError, no_color: bool) -> String {
    let prefix = if no_color { "[ERROR]" } else { "❌" };
    let category = err.category();
    let msg = match err {
        CliError::ConfigFile { msg, .. } => msg,
        CliError::NetworkError { msg, .. } => msg,
        CliError::PartialSuccess {
            success, failed, ..
        } => &format!("{success} exitosos, {failed} fallidos"),
        CliError::PreflightFailed { msg, .. } => msg,
    };
    let suggestion = err.suggestion();

    format!("{prefix} {category}\n  {msg}\n  Sugerencia: {suggestion}")
}

// ============================================================================
// T-051: CliExit enum with Termination trait
// ============================================================================

/// Exit codes following sysexits convention:
/// 0 = success, 64 = usage error, 65 = data format error, 69 = service unavailable (network/partial),
/// 74 = I/O error, 76 = protocol error, 77 = forbidden (robots.txt), 78 = config error
#[derive(Debug, Clone, PartialEq)]
pub enum CliExit {
    /// Exit 0 — everything OK
    Success,
    /// Exit 64 — bad usage / input
    UsageError(String),
    /// Exit 65 — data format error (malformed XML, etc.)
    DataFormatError(String),
    /// Exit 69 — network / service unavailable
    NetworkError(String),
    /// Exit 74 — I/O error
    IoError(String),
    /// Exit 76 — protocol error
    ProtocolError(String),
    /// Exit 77 — all URLs blocked by robots.txt
    Forbidden(String),
    /// Exit 78 — configuration error
    ConfigError(String),
    /// Exit 2 — no URLs discovered from sitemaps (technical success, null result)
    EmptyDiscovery(String),
    /// Exit 3 — all scrapers failed on discovered URLs
    ScraperFailure(String),
    /// Exit 69 — some URLs succeeded, some failed
    PartialSuccess {
        /// Number of successfully scraped URLs
        success: usize,
        /// Number of failed URLs
        failed: usize,
    },
}

// ============================================================================
// Error Classification Matrix — class → exit mapping (CLI boundary only)
//
// Contract: docs/error-classification-matrix.md (ID 261bdb66-197e-420f-a73b-
// 66c0e889102d), sections "Default exit codes by class", "Typed overrides over
// class defaults", and "Special cell — Cancelled". Exit-code knowledge stays
// OUT of the domain: classification lives in `CrawlError::classify()`, this is
// the only place that turns a class into an exit code.
// ============================================================================

/// Default CLI exit code for an [`ErrorClass`], per the matrix row
/// "Default exit codes by class".
///
/// Returns `None` for [`ErrorClass::PermanentFatal`] and
/// [`ErrorClass::DomainRecoverable`] because neither has a single default:
/// `PermanentFatal` is variant-dependent (64/65/69/76/77/78 per contract rows)
/// and `DomainRecoverable` depends on run outcome (0 if any item succeeded,
/// 65 if ALL items failed). Forcing either onto one code would misreport.
#[must_use]
pub const fn default_exit_code_for_class(class: ErrorClass) -> Option<u8> {
    match class {
        ErrorClass::TransientRetriable | ErrorClass::TransientBackoff => Some(EXIT_UNAVAILABLE),
        // An internal bug is a job failure, NEVER UsageError 64.
        ErrorClass::InternalFatal => Some(EXIT_SCRAPER_FAILURE),
        ErrorClass::PermanentFatal | ErrorClass::DomainRecoverable => None,
    }
}

/// Build the default [`CliExit`] for an [`ErrorClass`] with a caller-supplied
/// message. See [`default_exit_code_for_class`] for why the two
/// variant-dependent classes return `None`.
#[must_use]
pub fn cli_exit_for_class(class: ErrorClass, message: impl Into<String>) -> Option<CliExit> {
    match class {
        ErrorClass::TransientRetriable | ErrorClass::TransientBackoff => {
            Some(CliExit::NetworkError(message.into()))
        },
        ErrorClass::InternalFatal => Some(CliExit::ScraperFailure(message.into())),
        ErrorClass::PermanentFatal | ErrorClass::DomainRecoverable => None,
    }
}

/// Typed override — all URLs blocked by robots.txt / WAF → [`CliExit::Forbidden`]
/// (exit 77). Caller lacks permission; that is not a service fault.
///
/// Fires only when NOTHING was scraped and NOTHING failed, mirroring the
/// routing guard in `orchestrator::report_phase`; this function is the
/// canonical form so the orchestrator can adopt it without behavior change.
#[must_use]
pub fn forbidden_exit_when_all_blocked(
    results_len: usize,
    failures_len: usize,
    blocked: usize,
) -> Option<CliExit> {
    (results_len == 0 && failures_len == 0 && blocked > 0).then(|| {
        CliExit::Forbidden(format!(
            "{blocked} URL(s) bloqueadas por robots.txt. Usa --ignore-robots para omitir esta verificación."
        ))
    })
}

/// Typed override — extraction failures → [`CliExit::DataFormatError`] (exit 65).
/// Content-quality failure, not network: pages were fetched but carried no
/// usable content.
///
/// Canonical form of the typed `matches!` routing in
/// `orchestrator::data_format_error_for_extraction_failed`. Returns `None`
/// when no failure is an extraction failure.
#[must_use]
pub fn data_format_error_exit_when_extraction_failed(
    failures: &[(String, ScraperError)],
) -> Option<CliExit> {
    let extraction_failed = failures
        .iter()
        .filter(|(_, e)| matches!(e, ScraperError::ExtractionFailed { .. }))
        .count();
    (extraction_failed > 0).then(|| {
        CliExit::DataFormatError(format!(
            "extracción sin contenido útil: {extraction_failed} URL(s) devolvieron contenido insuficiente o requieren renderizado de JavaScript"
        ))
    })
}

/// Typed override — sitemap empty / sitemap not found →
/// [`CliExit::EmptyDiscovery`] (exit 2). Technical success with a null result,
/// not an operational failure.
#[must_use]
pub fn empty_discovery_exit_for(error: &ScraperError) -> Option<CliExit> {
    matches!(
        error,
        ScraperError::SitemapEmpty | ScraperError::SitemapNotFound(_)
    )
    .then(|| CliExit::EmptyDiscovery(format!("no URLs discovered: {error}")))
}

/// Typed override — config errors ([`ScraperError::Config`] /
/// [`ScraperError::H2Config`]) → [`CliExit::ConfigError`] (exit 78, EX_CONFIG).
#[must_use]
pub fn config_error_exit_for(error: &ScraperError) -> Option<CliExit> {
    matches!(error, ScraperError::Config(_) | ScraperError::H2Config(_))
        .then(|| CliExit::ConfigError(error.to_string()))
}

/// Typed override — permanent I/O errors → [`CliExit::IoError`] (exit 74,
/// EX_IOERR).
///
/// Transient I/O kinds (`Interrupted`, `WouldBlock`, `TimedOut`) return `None`:
/// they classify [`ErrorClass::TransientRetriable`] and keep the class default
/// (exit 69) instead of the permanent-Io override.
#[must_use]
pub fn permanent_io_error_exit_for(error: &std::io::Error) -> Option<CliExit> {
    if matches!(
        error.kind(),
        std::io::ErrorKind::Interrupted
            | std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
    ) {
        None
    } else {
        Some(CliExit::IoError(error.to_string()))
    }
}

/// Special cell — Cancelled. Cooperative cancellation is a control signal, NOT
/// an operational failure: intercepted BEFORE any classification-based routing,
/// it yields [`CliExit::Success`] (exit 0).
///
/// The domain's defensive fallback (`CrawlError::Cancelled → InternalFatal`)
/// only covers signals escaping through unexpected paths; this function is the
/// primary interception point at the CLI boundary.
#[must_use]
pub fn cancelled_exit(cancelled: bool) -> Option<CliExit> {
    cancelled.then_some(CliExit::Success)
}

impl std::process::Termination for CliExit {
    fn report(self) -> ExitCode {
        match self {
            CliExit::Success => ExitCode::from(EXIT_SUCCESS),
            CliExit::UsageError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_USAGE_ERROR)
            },
            CliExit::DataFormatError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_DATA_ERROR)
            },
            CliExit::NetworkError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_UNAVAILABLE)
            },
            CliExit::IoError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_IO_ERROR)
            },
            CliExit::ProtocolError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_PROTOCOL)
            },
            CliExit::Forbidden(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_FORBIDDEN)
            },
            CliExit::ConfigError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_CONFIG)
            },
            CliExit::EmptyDiscovery(msg) => {
                eprintln!("Warning: {msg}");
                ExitCode::from(EXIT_EMPTY_DISCOVERY)
            },
            CliExit::ScraperFailure(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_SCRAPER_FAILURE)
            },
            CliExit::PartialSuccess { .. } => ExitCode::from(EXIT_UNAVAILABLE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorClass;
    use std::process::Termination;

    // UT-01: ConfigFile formatting
    #[test]
    fn test_format_cli_error_config_file() {
        let err = CliError::ConfigFile {
            msg: "invalid TOML".into(),
            suggestion: "Check syntax".into(),
        };
        let formatted = format_cli_error(&err, false);
        // CliError::ConfigFile renders its category in Spanish (user-facing).
        assert!(formatted.contains("Configuración"));
        assert!(formatted.contains("invalid TOML"));
        assert!(formatted.contains("Check syntax"));
    }

    // UT-01 (no_color): ConfigFile formatting without emoji
    #[test]
    fn test_format_cli_error_config_file_no_color() {
        let err = CliError::ConfigFile {
            msg: "invalid TOML".into(),
            suggestion: "Check syntax".into(),
        };
        let formatted = format_cli_error(&err, true);
        assert!(formatted.contains("[ERROR]"));
        assert!(!formatted.contains("❌"));
    }

    // UT-02: NetworkError formatting
    #[test]
    fn test_format_cli_error_network_error() {
        let err = CliError::NetworkError {
            msg: "connection refused".into(),
            suggestion: "Check your network".into(),
        };
        let formatted = format_cli_error(&err, false);
        // CliError::NetworkError renders its category in Spanish (user-facing).
        assert!(formatted.contains("Red"));
        assert!(formatted.contains("connection refused"));
    }

    // UT-03: PartialSuccess exit code
    #[test]
    fn test_cli_exit_partial_success_exit_code() {
        let exit = CliExit::PartialSuccess {
            success: 5,
            failed: 2,
        };
        let code = exit.report();
        assert_eq!(code, ExitCode::from(69));
    }

    // UT-04: Success exit code
    #[test]
    fn test_cli_exit_success_exit_code() {
        let exit = CliExit::Success;
        let code = exit.report();
        assert_eq!(code, ExitCode::from(0));
    }

    // UT-05: ConfigError exit code
    #[test]
    fn test_cli_exit_config_error_exit_code() {
        let exit = CliExit::ConfigError("bad config".into());
        let code = exit.report();
        assert_eq!(code, ExitCode::from(78));
    }

    // UT-06: NetworkError exit code
    #[test]
    fn test_cli_exit_network_error_exit_code() {
        let exit = CliExit::NetworkError("timeout".into());
        let code = exit.report();
        assert_eq!(code, ExitCode::from(69));
    }

    // T-1.1: Named constants are accessible and correct
    #[test]
    fn test_exit_code_constants_values() {
        assert_eq!(EXIT_SUCCESS, 0);
        assert_eq!(EXIT_EMPTY_DISCOVERY, 2);
        assert_eq!(EXIT_SCRAPER_FAILURE, 3);
        assert_eq!(EXIT_USAGE_ERROR, 64);
        assert_eq!(EXIT_DATA_ERROR, 65);
        assert_eq!(EXIT_UNAVAILABLE, 69);
        assert_eq!(EXIT_IO_ERROR, 74);
        assert_eq!(EXIT_PROTOCOL, 76);
        assert_eq!(EXIT_FORBIDDEN, 77);
        assert_eq!(EXIT_CONFIG, 78);
    }

    // T-1.2: DataFormatError variant maps to exit 65
    #[test]
    fn test_cli_exit_data_format_error_exit_code() {
        let exit = CliExit::DataFormatError("malformed XML".into());
        let code = exit.report();
        assert_eq!(code, ExitCode::from(EXIT_DATA_ERROR));
    }

    // T-1.3: EmptyDiscovery variant maps to exit 2
    #[test]
    fn test_cli_exit_empty_discovery_exit_code() {
        let exit = CliExit::EmptyDiscovery("No URLs found".into());
        let code = exit.report();
        assert_eq!(code, ExitCode::from(EXIT_EMPTY_DISCOVERY));
    }

    // T-1.3: Forbidden variant maps to exit 77
    #[test]
    fn test_cli_exit_forbidden_exit_code() {
        let exit = CliExit::Forbidden("all URLs blocked by robots.txt".into());
        let code = exit.report();
        assert_eq!(code, ExitCode::from(EXIT_FORBIDDEN));
    }

    // ====================================================================
    // Error Classification Matrix — Sprint 1-2 P0-err final DoD item.
    // Contract: docs/error-classification-matrix.md (ID 261bdb66).
    // ====================================================================

    // DoD: each ErrorClass maps to its documented default exit code.
    #[test]
    fn default_exit_code_transient_retriable_is_69() {
        assert_eq!(
            default_exit_code_for_class(ErrorClass::TransientRetriable),
            Some(EXIT_UNAVAILABLE)
        );
    }

    #[test]
    fn default_exit_code_transient_backoff_is_69() {
        assert_eq!(
            default_exit_code_for_class(ErrorClass::TransientBackoff),
            Some(EXIT_UNAVAILABLE)
        );
    }

    #[test]
    fn default_exit_code_internal_fatal_is_scraper_failure_never_usage() {
        // InternalFatal must map to ScraperFailure (3), NEVER UsageError 64:
        // an internal bug is not user error.
        assert_eq!(
            default_exit_code_for_class(ErrorClass::InternalFatal),
            Some(EXIT_SCRAPER_FAILURE)
        );
    }

    #[test]
    fn permanent_fatal_has_no_single_default_exit() {
        // Variant-dependent per contract rows (64/65/69/76/77/78): forcing a
        // single code here would be wrong, so the mapping honestly returns None.
        assert_eq!(
            default_exit_code_for_class(ErrorClass::PermanentFatal),
            None
        );
    }

    #[test]
    fn domain_recoverable_has_no_single_default_exit() {
        // Contract: 0 if any item succeeded; 65 if ALL items failed — decided
        // by run outcome, not by the class alone.
        assert_eq!(
            default_exit_code_for_class(ErrorClass::DomainRecoverable),
            None
        );
    }

    #[test]
    fn cli_exit_for_class_transient_maps_to_network_error_variant() {
        let exit = cli_exit_for_class(ErrorClass::TransientRetriable, "boom");
        assert!(matches!(exit, Some(CliExit::NetworkError(_))));
        assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_UNAVAILABLE));
    }

    #[test]
    fn cli_exit_for_class_internal_fatal_maps_to_scraper_failure_variant() {
        let exit = cli_exit_for_class(ErrorClass::InternalFatal, "bug");
        assert!(matches!(exit, Some(CliExit::ScraperFailure(_))));
        assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_SCRAPER_FAILURE));
    }

    #[test]
    fn cli_exit_for_class_variant_dependent_classes_return_none() {
        assert_eq!(cli_exit_for_class(ErrorClass::PermanentFatal, "x"), None);
        assert_eq!(cli_exit_for_class(ErrorClass::DomainRecoverable, "x"), None);
    }

    // ---- Typed overrides over class defaults ----

    #[test]
    fn forbidden_override_when_all_blocked_maps_to_77() {
        let exit = forbidden_exit_when_all_blocked(0, 0, 5);
        assert!(matches!(exit, Some(CliExit::Forbidden(_))));
        assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_FORBIDDEN));
    }

    #[test]
    fn forbidden_override_requires_nothing_scraped_or_failed() {
        // Any real failure or any scraped page keeps historical routing (None).
        assert_eq!(forbidden_exit_when_all_blocked(1, 0, 5), None);
        assert_eq!(forbidden_exit_when_all_blocked(0, 2, 5), None);
        assert_eq!(forbidden_exit_when_all_blocked(0, 0, 0), None);
    }

    #[test]
    fn data_format_override_when_extraction_failed_maps_to_65() {
        let failures = vec![(
            "https://example.com".to_string(),
            crate::error::ScraperError::ExtractionFailed {
                url: "https://example.com".into(),
                reason: "empty shell".into(),
            },
        )];
        let exit = data_format_error_exit_when_extraction_failed(&failures);
        assert!(matches!(exit, Some(CliExit::DataFormatError(_))));
        assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_DATA_ERROR));
    }

    #[test]
    fn data_format_override_ignores_non_extraction_failures() {
        let failures = vec![(
            "https://example.com".to_string(),
            crate::error::ScraperError::Network(Box::<std::io::Error>::new(std::io::Error::other(
                "reset",
            ))),
        )];
        assert_eq!(
            data_format_error_exit_when_extraction_failed(&failures),
            None
        );
        assert_eq!(data_format_error_exit_when_extraction_failed(&[]), None);
    }

    #[test]
    fn empty_discovery_override_for_sitemap_variants_maps_to_2() {
        for err in [
            crate::error::ScraperError::SitemapEmpty,
            crate::error::ScraperError::SitemapNotFound("none".into()),
        ] {
            let exit = empty_discovery_exit_for(&err);
            assert!(matches!(exit, Some(CliExit::EmptyDiscovery(_))));
            assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_EMPTY_DISCOVERY));
        }
    }

    #[test]
    fn empty_discovery_override_ignores_other_errors() {
        let err = crate::error::ScraperError::Network(Box::<std::io::Error>::new(
            std::io::Error::other("reset"),
        ));
        assert_eq!(empty_discovery_exit_for(&err), None);
    }

    #[test]
    fn config_override_maps_to_78() {
        for err in [
            crate::error::ScraperError::Config("bad key".into()),
            crate::error::ScraperError::H2Config("bad alpn".into()),
        ] {
            let exit = config_error_exit_for(&err);
            assert!(matches!(exit, Some(CliExit::ConfigError(_))));
            assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_CONFIG));
        }
    }

    #[test]
    fn config_override_ignores_non_config_errors() {
        let err = crate::error::ScraperError::InvalidUrl("nope".into());
        assert_eq!(config_error_exit_for(&err), None);
    }

    #[test]
    fn permanent_io_override_maps_to_74() {
        let exit = permanent_io_error_exit_for(&std::io::Error::from(std::io::ErrorKind::NotFound));
        assert!(matches!(exit, Some(CliExit::IoError(_))));
        assert_eq!(exit.unwrap().report(), ExitCode::from(EXIT_IO_ERROR));
    }

    #[test]
    fn transient_io_kinds_keep_the_class_default_instead_of_74() {
        // Rows 21: Interrupted / WouldBlock / TimedOut are TransientRetriable;
        // the class default (69) applies, so no typed override fires.
        for kind in [
            std::io::ErrorKind::Interrupted,
            std::io::ErrorKind::WouldBlock,
            std::io::ErrorKind::TimedOut,
        ] {
            let err = std::io::Error::from(kind);
            assert_eq!(permanent_io_error_exit_for(&err), None);
        }
    }

    // ---- Special cell — Cancelled ----

    #[test]
    fn cancelled_maps_to_success_exit_0_before_classification() {
        let exit = cancelled_exit(true).expect("cancellation must yield an exit");
        assert!(matches!(exit, CliExit::Success));
        assert_eq!(exit.report(), ExitCode::from(EXIT_SUCCESS));
    }

    #[test]
    fn non_cancelled_run_yields_no_cancelled_interception() {
        assert_eq!(cancelled_exit(false), None);
    }

    // T-1.4: All variants map to their named constants (exhaustive)
    #[test]
    fn test_all_variants_map_to_named_constants() {
        let cases: Vec<(CliExit, u8)> = vec![
            (CliExit::Success, EXIT_SUCCESS),
            (CliExit::UsageError("test".into()), EXIT_USAGE_ERROR),
            (CliExit::DataFormatError("test".into()), EXIT_DATA_ERROR),
            (CliExit::NetworkError("test".into()), EXIT_UNAVAILABLE),
            (CliExit::IoError("test".into()), EXIT_IO_ERROR),
            (CliExit::ProtocolError("test".into()), EXIT_PROTOCOL),
            (CliExit::ConfigError("test".into()), EXIT_CONFIG),
            (CliExit::EmptyDiscovery("test".into()), EXIT_EMPTY_DISCOVERY),
            (CliExit::ScraperFailure("test".into()), EXIT_SCRAPER_FAILURE),
            (CliExit::Forbidden("test".into()), EXIT_FORBIDDEN),
            (
                CliExit::PartialSuccess {
                    success: 1,
                    failed: 1,
                },
                EXIT_UNAVAILABLE,
            ),
        ];
        for (exit, expected_code) in cases {
            let code = exit.report();
            assert_eq!(
                code,
                ExitCode::from(expected_code),
                "Expected exit code {expected_code}"
            );
        }
    }
}
