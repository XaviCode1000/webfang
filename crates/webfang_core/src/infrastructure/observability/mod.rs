//! Observability Module
//!
//! Production-grade observability infrastructure:
//! - Structured JSON logging with file rotation
//! - FileTraceLayer: JSONL span/event traces for offline debugging
//! - Tokio console for runtime debugging
//!
//! # Tokio Console (Optional)
//!
//! For runtime observability, enable the `console` feature:
//! ```bash
//! RUSTFLAGS="--cfg tokio_unstable" cargo run --features console -- --url ...
//! ```
//!
//! Then in your code:
//! ```ignore
//! #[cfg(feature = "console")]
//! webfang_core::infrastructure::observability::init_console();
//! ```

pub mod error_logging;
pub mod file_trace_layer;
pub mod logging;
pub mod memory_probe;

/// Initialize tokio-console for runtime debugging
///
/// # Requires
/// - RUSTFLAGS="--cfg tokio_unstable" at compile time
/// - Feature flag `console` enabled
///
/// # Note
/// Only available when compiled with `console` feature.
/// Without the feature, this function is a no-op.
#[cfg(feature = "console")]
pub fn init_console() {
    console_subscriber::init();
}

/// Placeholder when console feature is not enabled
#[cfg(not(feature = "console"))]
pub fn init_console() {
    // No-op - console not enabled
}

pub use error_logging::{log_classified_error, log_scrape_error};
pub use file_trace_layer::FileTraceLayer;
pub use logging::{init_json_logging, init_json_logging_dual, LogFormat, LogGuard};
