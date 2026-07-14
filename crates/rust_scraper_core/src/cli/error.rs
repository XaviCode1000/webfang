//! CLI Error Types and Exit Codes
//!
//! T-050: CliError enum with thiserror
//! T-051: CliExit enum with Termination trait for sysexits codes

use std::process::ExitCode;
use thiserror::Error;

// ============================================================================
// T-050: CliError enum
// ============================================================================

/// Categorized CLI errors with user-friendly suggestions.
#[derive(Error, Debug)]
pub enum CliError {
    #[error("Configuration: {msg}\n  Suggestion: {suggestion}")]
    ConfigFile { msg: String, suggestion: String },

    #[error("Network: {msg}\n  Suggestion: {suggestion}")]
    NetworkError { msg: String, suggestion: String },

    #[error("Partial Success: {success} succeeded, {failed} failed\n  Suggestion: {suggestion}")]
    PartialSuccess {
        success: u32,
        failed: u32,
        suggestion: String,
    },

    #[error("Preflight Check Failed: {msg}\n  Suggestion: {suggestion}")]
    PreflightFailed { msg: String, suggestion: String },
}

impl CliError {
    /// Get the human-readable category name for this error.
    pub fn category(&self) -> &'static str {
        match self {
            CliError::ConfigFile { .. } => "Configuration",
            CliError::NetworkError { .. } => "Network",
            CliError::PartialSuccess { .. } => "Partial Success",
            CliError::PreflightFailed { .. } => "Preflight Check Failed",
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
        } => &format!("{success} succeeded, {failed} failed"),
        CliError::PreflightFailed { msg, .. } => msg,
    };
    let suggestion = err.suggestion();

    format!("{prefix} {category}\n  {msg}\n  Suggestion: {suggestion}")
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
    /// Exit 69 — some URLs succeeded, some failed
    PartialSuccess { success: usize, failed: usize },
}

impl std::process::Termination for CliExit {
    fn report(self) -> ExitCode {
        match self {
            CliExit::Success => ExitCode::from(0),
            CliExit::UsageError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(64)
            },
            CliExit::NetworkError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(69)
            },
            CliExit::IoError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(74)
            },
            CliExit::ProtocolError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(76)
            },
            CliExit::ConfigError(msg) => {
                eprintln!("Error: {msg}");
                ExitCode::from(78)
            },
            CliExit::PartialSuccess { .. } => ExitCode::from(69),
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
}
