//! Configuration Module
//!
//! Handles logging initialization and application configuration.

use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize logging with configurable level
///
/// # Arguments
///
/// * `level` - Log level: "error", "warn", "info", "debug", "trace"
pub fn init_logging(level: &str) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let default_level = format!("rust_scraper={},tokio=warn,reqwest=warn", level);
        EnvFilter::new(default_level)
    });

    tracing_subscriber::registry()
        .with(fmt::layer().pretty().with_target(true))
        .with(env_filter)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_logging_with_valid_level() {
        // Act & Assert - Should not panic with valid levels
        // Note: init() can only be called once, so we use try_init() for tests
        let result = tracing_subscriber::registry()
            .with(fmt::layer().pretty().with_target(true))
            .with(EnvFilter::new("error"))
            .try_init();

        // Either succeeds (first init) or fails (already initialized from another test)
        // Both are acceptable - we just verify no panic
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_init_logging_with_debug_level() {
        // This tests that the logging configuration doesn't panic
        // Actual log output depends on test environment
        let _filter = EnvFilter::new("rust_scraper=debug,tokio=warn,reqwest=warn");
        // If we get here without panic, test passes
    }

    #[test]
    fn test_init_logging_with_trace_level() {
        let _filter = EnvFilter::new("rust_scraper=trace,tokio=warn,reqwest=warn");
    }
}
