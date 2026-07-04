//! JSON Logging Initialization Module
//!
//! Provides JSON-formatted logging with file rotation for production use.
//! Uses tracing-subscriber with json feature.
//!
//! Also provides async logging via AsyncLogWriter for non-blocking writes.

use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
#[cfg(feature = "otel")]
use tracing_subscriber::Registry;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// Guard for JSON logging - ensures flush on drop (RAII)
///
/// Dropping this guard ensures all pending log writes are flushed
/// to the file before the application exits.
pub struct LogGuard {
    _guard: Option<WorkerGuard>,
}

impl LogGuard {
    /// Create a no-op guard (when not logging to file)
    fn no_op() -> Self {
        Self { _guard: None }
    }
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        // WorkerGuard automatically flushes on drop
        // If Some(_), logs are flushed. If None, no-op.
    }
}

/// Log format enum for CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogFormat {
    /// Human-readable text format (default)
    #[default]
    Text,
    /// JSON format for machine parsing
    Json,
}

impl std::str::FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            _ => Err(format!("Invalid log format: {s}. Valid: text, json")),
        }
    }
}

impl std::fmt::Display for LogFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Self::Text => "text",
                Self::Json => "json",
            }
        )
    }
}

/// Initialize JSON logging with file rotation.
///
/// # Arguments
///
/// * `level` - Log level: "error", "warn", "info", "debug", "trace"
/// * `log_dir` - Optional directory for log files. If None, logs to stderr only.
/// * `app_name` - Application name for log file naming (default: "rust_scraper")
///
/// # Returns
///
/// `Ok(LogGuard)` on success - keep this guard alive until program exit.
/// The guard ensures all logs are flushed when dropped.
///
/// # Example
///
/// ```rust,ignore
/// fn main() {
///     let _guard = init_json_logging("info", Some("/var/log"), "myapp").unwrap();
///     // ... application runs ...
///     // Logs are flushed when _guard is dropped at end of main
/// }
/// ```
#[cfg(not(feature = "otel"))]
pub fn init_json_logging(
    level: &str,
    log_dir: Option<&Path>,
    app_name: &str,
) -> anyhow::Result<LogGuard> {
    init_json_logging_dual(level, false, false, log_dir, app_name)
}

/// Initialize JSON logging with file rotation (otel-enabled variant).
#[cfg(feature = "otel")]
pub fn init_json_logging(
    level: &str,
    log_dir: Option<&Path>,
    app_name: &str,
) -> anyhow::Result<LogGuard> {
    init_json_logging_dual(level, false, false, log_dir, app_name, None)
}

/// Extended JSON logging with quiet mode and no-color support.
///
/// # Arguments
///
/// * `level` - Log level
/// * `quiet` - If true, only warn+ output
/// * `no_color` - If true, disable ANSI colors
/// * `log_dir` - Optional directory for log files
/// * `app_name` - Application name for log file naming
#[cfg(not(feature = "otel"))]
pub fn init_json_logging_dual(
    level: &str,
    quiet: bool,
    no_color: bool,
    log_dir: Option<&Path>,
    app_name: &str,
) -> anyhow::Result<LogGuard> {
    let filter = if quiet {
        EnvFilter::new("rust_scraper=warn,tokio=warn,reqwest=warn")
    } else {
        EnvFilter::new(format!("rust_scraper={level},tokio=warn,reqwest=warn"))
    };

    // Build subscriber layers
    let subscriber = tracing_subscriber::registry().with(filter);

    #[cfg(feature = "dev-tracing")]
    let subscriber = {
        use tracing_tree::HierarchicalLayer;
        subscriber.with(
            HierarchicalLayer::new(2)
                .with_targets(true)
                .with_bracketed_fields(true),
        )
    };

    // Text layer for stderr (always)
    let text_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(!no_color)
        .with_target(true)
        .pretty();

    let subscriber = subscriber.with(text_layer);

    // JSON file layer if log_dir provided
    if let Some(dir) = log_dir {
        let file_appender =
            RollingFileAppender::new(Rotation::DAILY, dir, format!("{app_name}.log"));
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        let log_guard = LogGuard {
            _guard: Some(guard),
        };

        let json_layer = fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(true)
            .json();

        subscriber.with(json_layer).try_init().ok();
        Ok(log_guard)
    } else {
        subscriber.try_init().ok();
        Ok(LogGuard::no_op())
    }
}

/// Extended JSON logging with quiet mode, no-color support, and optional OTel layer.
///
/// When the `otel` feature is enabled, accepts an optional `OpenTelemetryLayer`
/// that is inserted into the subscriber chain after the EnvFilter and before
/// the fmt layers. When `None`, behavior is identical to the non-otel build.
#[cfg(feature = "otel")]
pub fn init_json_logging_dual(
    level: &str,
    quiet: bool,
    no_color: bool,
    log_dir: Option<&Path>,
    app_name: &str,
    otel_layer: Option<
        tracing_opentelemetry::OpenTelemetryLayer<Registry, opentelemetry_sdk::trace::Tracer>,
    >,
) -> anyhow::Result<LogGuard> {
    let filter = if quiet {
        EnvFilter::new("rust_scraper=warn,tokio=warn,reqwest=warn")
    } else {
        EnvFilter::new(format!("rust_scraper={level},tokio=warn,reqwest=warn"))
    };

    // Build subscriber: OTel layer must be added directly on Registry, before EnvFilter
    let subscriber = tracing_subscriber::registry().with(otel_layer).with(filter);

    #[cfg(feature = "dev-tracing")]
    let subscriber = {
        use tracing_tree::HierarchicalLayer;
        subscriber.with(
            HierarchicalLayer::new(2)
                .with_targets(true)
                .with_bracketed_fields(true),
        )
    };

    // Text layer for stderr (always)
    let text_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(!no_color)
        .with_target(true)
        .pretty();

    let subscriber = subscriber.with(text_layer);

    // JSON file layer if log_dir provided
    if let Some(dir) = log_dir {
        let file_appender =
            RollingFileAppender::new(Rotation::DAILY, dir, format!("{app_name}.log"));
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        let log_guard = LogGuard {
            _guard: Some(guard),
        };

        let json_layer = fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(true)
            .json();

        subscriber.with(json_layer).try_init().ok();
        Ok(log_guard)
    } else {
        subscriber.try_init().ok();
        Ok(LogGuard::no_op())
    }
}

/// Initialize OpenTelemetry tracing (stub for future implementation).
///
/// Currently returns Ok(()) - full OpenTelemetry integration deferred per proposal scope.
pub fn init_otel_tracing() -> anyhow::Result<()> {
    // TODO: Implement OpenTelemetry exporter
    // For now, this is a stub that allows the code to compile
    // Full distributed tracing with W3C TraceContext is deferred
    tracing::debug!("OpenTelemetry tracing initialized (stub)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_log_format_from_str() {
        assert_eq!(LogFormat::from_str("text").unwrap(), LogFormat::Text);
        assert_eq!(LogFormat::from_str("json").unwrap(), LogFormat::Json);
        assert_eq!(LogFormat::from_str("TEXT").unwrap(), LogFormat::Text);
        assert_eq!(LogFormat::from_str("JSON").unwrap(), LogFormat::Json);
    }

    #[test]
    fn test_log_format_from_str_invalid() {
        assert!(LogFormat::from_str("invalid").is_err());
    }

    #[test]
    fn test_log_format_display() {
        assert_eq!(LogFormat::Text.to_string(), "text");
        assert_eq!(LogFormat::Json.to_string(), "json");
    }

    #[test]
    fn test_init_json_logging_default() {
        // Should not panic - initializes with default settings
        let result = init_json_logging("info", None, "test-app");
        assert!(result.is_ok());
    }

    #[test]
    #[ignore] // Ignored: tracing global subscriber may already be set in test context
    fn test_init_json_logging_with_temp_dir() {
        let temp_dir = std::env::temp_dir();
        // Note: tracing subscriber may already be initialized in test context
        // This test verifies the function works when called with a temp dir
        let _ = init_json_logging("info", Some(&temp_dir), "test-app");

        // Clean up log file if created
        let log_file = temp_dir.join("test-app.log");
        let _ = std::fs::remove_file(log_file);
    }

    #[test]
    fn test_init_otel_tracing() {
        let result = init_otel_tracing();
        assert!(result.is_ok());
    }

    #[cfg(feature = "otel")]
    mod otel_layer {
        use super::*;

        #[test]
        fn test_init_json_logging_dual_accepts_none_layer() {
            let result = init_json_logging_dual("info", false, false, None, "test-app", None);
            assert!(
                result.is_ok(),
                "init_json_logging_dual with None OTel layer must succeed"
            );
        }

        #[test]
        fn test_init_json_logging_dual_accepts_some_layer() {
            let config = crate::infrastructure::observability::otel::OtelConfig::from_env();
            let (_guard, layer) =
                crate::infrastructure::observability::otel::init_otel_tracing(config).unwrap();

            let result =
                init_json_logging_dual("info", false, false, None, "test-app", Some(layer));
            assert!(
                result.is_ok(),
                "init_json_logging_dual with OTel layer must succeed"
            );
        }
    }
}
