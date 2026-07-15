//! Observability Module
//!
//! Production-grade observability infrastructure:
//! - Structured JSON logging with file rotation
//! - OpenTelemetry tracing and metrics (feature-gated)
//! - Tokio console for runtime debugging
//!
//! # Features
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `otel` | OpenTelemetry distributed tracing via OTLP HTTP/protobuf |
//! | `otel-metrics` | Extends `otel` with metric instruments and OTLP metric export |
//!
//! # Tokio Console (Optional)
//!
//! For runtime observability, enable the `console` feature:
//! ```bash
//! RUSTFLAGS="--cfg tokio_unstable" cargo run --features console -- --url ...
//! ```
//!
//! Then in your code:
//! ```rust
//! #[cfg(feature = "console")]
//! webfang::infrastructure::observability::init_console();
//! ```

pub mod file_trace_layer;
pub mod logging;
#[cfg(feature = "otel")]
pub mod otel;

#[cfg(feature = "otel-metrics")]
pub mod metrics_instruments;

#[cfg(feature = "otel")]
pub mod trace_correlation;

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

pub use file_trace_layer::FileTraceLayer;
pub use logging::{init_json_logging, init_json_logging_dual, LogFormat, LogGuard};

#[cfg(feature = "otel")]
pub use otel::{OtelConfig, OtelGuard};

#[cfg(feature = "otel-metrics")]
pub use otel::init_otel_metrics;

#[cfg(feature = "otel")]
pub use trace_correlation::trace_correlation_layer;
