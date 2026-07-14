//! CLI Error Types and Exit Codes
//!
//! T-050: CliError enum with thiserror
//! T-051: CliExit enum with Termination trait for sysexits codes

use std::process::ExitCode;
use thiserror::Error;

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
/// Exit 69 — Infrastructure/network failure (sysexits EX_UNAVAILABLE).
pub const EXIT_UNAVAILABLE: u8 = 69;
/// Exit 74 — File I/O error (sysexits EX_IOERR).
pub const EXIT_IO_ERROR: u8 = 74;
/// Exit 76 — Protocol error (sysexits EX_PROTOCOL).
pub const EXIT_PROTOCOL: u8 = 76;
/// Exit 78 — Configuration error (sysexits EX_CONFIG).
pub const EXIT_CONFIG: u8 = 78;

// ============================================================================
// T-050: CliError enum
// ============================================================================

/// Categorized CLI errors with user-friendly suggestions.
#[derive(Error, Debug)]
pub enum CliError {
    #[error("Configuración: {msg}\n  Sugerencia: {suggestion}")]
    ConfigFile { msg: String, suggestion: String },

    #[error("Red: {msg}\n  Sugerencia: {suggestion}")]
    NetworkError { msg: String, suggestion: String },

    #[error("Éxito parcial: {success} exitosos, {failed} fallidos\n  Sugerencia: {suggestion}")]
    PartialSuccess {
        success: u32,
        failed: u32,
        suggestion: String,
    },

    #[error("Verificación previa fallida: {msg}\n  Sugerencia: {suggestion}")]
    PreflightFailed { msg: String, suggestion: String },
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
/// 0 = success, 64 = usage error, 69 = service unavailable (network/partial),
/// 74 = I/O error, 76 = protocol error, 78 = config error
#[derive(Debug)]
pub enum CliExit {
    /// Exit 0 — everything OK
    Success,
    /// Exit 64 — bad usage / input
    UsageError(String),
    /// Exit 69 — network / service unavailable
    NetworkError(String),
    /// Exit 74 — I/O error
    IoError(String),
    /// Exit 76 — protocol error
    ProtocolError(String),
    /// Exit 78 — configuration error
    ConfigError(String),
    /// Exit 2 — no URLs discovered from sitemaps (technical success, null result)
    EmptyDiscovery(String),
    /// Exit 69 — some URLs succeeded, some failed
    PartialSuccess { success: usize, failed: usize },
}

impl std::process::Termination for CliExit {
    fn report(self) -> ExitCode {
        match self {
            CliExit::Success => ExitCode::from(EXIT_SUCCESS),
            CliExit::UsageError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_USAGE_ERROR)
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
            CliExit::ConfigError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(EXIT_CONFIG)
            },
            CliExit::EmptyDiscovery(msg) => {
                eprintln!("Warning: {msg}");
                ExitCode::from(EXIT_EMPTY_DISCOVERY)
            },
            CliExit::PartialSuccess { .. } => ExitCode::from(EXIT_UNAVAILABLE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Termination;

    // UT-01: ConfigFile formatting
    #[test]
    fn test_format_cli_error_config_file() {
        let err = CliError::ConfigFile {
            msg: "invalid TOML".into(),
            suggestion: "Check syntax".into(),
        };
        let formatted = format_cli_error(&err, false);
        assert!(formatted.contains("Configuration"));
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
        assert!(formatted.contains("Network"));
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
        assert_eq!(EXIT_UNAVAILABLE, 69);
        assert_eq!(EXIT_IO_ERROR, 74);
        assert_eq!(EXIT_PROTOCOL, 76);
        assert_eq!(EXIT_CONFIG, 78);
    }

    // T-1.3: EmptyDiscovery variant maps to exit 2
    #[test]
    fn test_cli_exit_empty_discovery_exit_code() {
        let exit = CliExit::EmptyDiscovery("No URLs found".into());
        let code = exit.report();
        assert_eq!(code, ExitCode::from(EXIT_EMPTY_DISCOVERY));
    }

    // T-1.4: All variants map to their named constants (exhaustive)
    #[test]
    fn test_all_variants_map_to_named_constants() {
        let cases: Vec<(CliExit, u8)> = vec![
            (CliExit::Success, EXIT_SUCCESS),
            (CliExit::UsageError("test".into()), EXIT_USAGE_ERROR),
            (CliExit::NetworkError("test".into()), EXIT_UNAVAILABLE),
            (CliExit::IoError("test".into()), EXIT_IO_ERROR),
            (CliExit::ProtocolError("test".into()), EXIT_PROTOCOL),
            (CliExit::ConfigError("test".into()), EXIT_CONFIG),
            (CliExit::EmptyDiscovery("test".into()), EXIT_EMPTY_DISCOVERY),
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
                "Expected exit code {}",
                expected_code
            );
        }
    }
}
