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
