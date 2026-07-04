//! OpenTelemetry Distributed Tracing
//!
//! Feature-gated behind `otel`. Provides OTLP HTTP/protobuf trace export
//! with configurable endpoint and service naming.
//!
//! # Environment Variables
//!
//! | Variable | Default | Description |
//! |----------|---------|-------------|
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318` | OTLP HTTP collector endpoint |
//! | `OTEL_EXPORTER_OTLP_HEADERS` | `""` | Extra headers (`k1=v1,k2=v2`) — e.g. Grafana Cloud auth |
//! | `OTEL_SERVICE_NAME` | `rust_scraper` | Service name in OTel resource |
//!
//! # Usage
//!
//! ```rust,ignore
//! use rust_scraper::infrastructure::observability::otel::{OtelConfig, init_otel_tracing};
//!
//! let config = OtelConfig::from_env();
//! let (guard, layer) = init_otel_tracing(config)?;
//! // pass layer to init_json_logging_dual(..., Some(layer))
//! // keep guard alive until program exit
//! ```

use std::env;
use std::path::PathBuf;

use opentelemetry::global;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_otlp::SpanExporter;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::Registry;

use super::file_trace_exporter::FileTraceExporter;

#[cfg(feature = "otel-metrics")]
use opentelemetry_otlp::MetricExporter;
#[cfg(feature = "otel-metrics")]
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
#[cfg(feature = "otel-metrics")]
use std::time::Duration;

/// OpenTelemetry configuration.
#[derive(Debug, Clone)]
pub struct OtelConfig {
    /// OTLP HTTP endpoint (default: `http://localhost:4318`)
    pub endpoint: String,
    /// Service name for resource attributes (default: `rust_scraper`)
    pub service_name: String,
    /// Optional path to write OTel spans as JSONL for offline debugging.
    /// When set, a synchronous file exporter is added alongside the OTLP exporter.
    pub trace_file: Option<PathBuf>,
}

impl OtelConfig {
    /// Create config from environment variables with defaults.
    ///
    /// Reads `OTEL_EXPORTER_OTLP_ENDPOINT` (default: `http://localhost:4318`),
    /// `OTEL_SERVICE_NAME` (default: `rust_scraper`), and
    /// `RUST_SCRAPER_TRACE_FILE` (optional).
    pub fn from_env() -> Self {
        Self {
            endpoint: env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:4318".to_string()),
            service_name: env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "rust_scraper".to_string()),
            trace_file: env::var("RUST_SCRAPER_TRACE_FILE")
                .ok()
                .map(PathBuf::from)
                .filter(|p| !p.as_os_str().is_empty()),
        }
    }

    /// Override the OTLP endpoint.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    /// Override the service name.
    #[must_use]
    pub fn with_service_name(mut self, name: impl Into<String>) -> Self {
        self.service_name = name.into();
        self
    }

    /// Set a file path for JSONL span export (offline debugging).
    #[must_use]
    pub fn with_trace_file(mut self, path: PathBuf) -> Self {
        self.trace_file = Some(path);
        self
    }
}

/// RAII guard for OpenTelemetry shutdown.
///
/// When dropped, flushes all pending spans from the `BatchSpanProcessor`
/// and shuts down the `MeterProvider` (if metrics are enabled).
/// Must be kept alive for the duration of the program.
pub struct OtelGuard {
    provider: Option<SdkTracerProvider>,
    #[cfg(feature = "otel-metrics")]
    meter_provider: Option<SdkMeterProvider>,
}

impl OtelGuard {
    fn new(provider: SdkTracerProvider) -> Self {
        Self {
            provider: Some(provider),
            #[cfg(feature = "otel-metrics")]
            meter_provider: None,
        }
    }

    #[cfg(feature = "otel-metrics")]
    fn with_meter_provider(mut self, meter_provider: SdkMeterProvider) -> Self {
        self.meter_provider = Some(meter_provider);
        self
    }

    /// Flush pending telemetry and shut down providers.
    ///
    /// Must be called **before** the Tokio runtime shuts down, and from
    /// within a running Tokio context. The PeriodicReader's reqwest HTTP
    /// call needs the Tokio runtime to make progress — using
    /// `tokio::time::sleep` instead of `std::thread::sleep` ensures the
    /// runtime stays responsive so the export can complete.
    pub async fn flush(&self) {
        if let Some(ref provider) = self.provider {
            if let Err(e) = provider.shutdown() {
                eprintln!("Warning: OTel trace shutdown failed: {e}");
            }
        }
        #[cfg(feature = "otel-metrics")]
        if let Some(ref meter) = self.meter_provider {
            // force_flush triggers an immediate export cycle; the
            // tokio::time::sleep yields the runtime so the PeriodicReader's
            // background thread (which uses futures_executor::block_on for
            // the blocking reqwest call) can complete.
            if let Err(e) = meter.force_flush() {
                eprintln!("Warning: OTel metrics force_flush failed: {e}");
            }
            // Give the PeriodicReader's background thread time to
            // collect and send via the blocking reqwest client.
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Err(e) = meter.shutdown() {
                eprintln!("Warning: OTel metrics shutdown failed: {e}");
            }
        }
    }
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Some(provider) = self.provider.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = provider.shutdown();
            }));
        }
        #[cfg(feature = "otel-metrics")]
        if let Some(meter) = self.meter_provider.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = meter.shutdown();
            }));
        }
    }
}

/// Initialize OpenTelemetry tracing with the given config.
///
/// Creates a `TracerProvider` with a `BatchSpanProcessor` backed by the
/// HTTP OTLP exporter, installs it as the global tracer provider, and
/// returns an `OpenTelemetryLayer` for integration into the tracing subscriber.
///
/// # Arguments
///
/// * `config` - OTel configuration (endpoint, service name)
///
/// # Returns
///
/// A tuple of `(OtelGuard, OpenTelemetryLayer)` where:
/// - The guard must be kept alive until program exit
/// - The layer is added to the tracing-subscriber Registry
pub fn init_otel_tracing(
    config: OtelConfig,
) -> anyhow::Result<(
    OtelGuard,
    OpenTelemetryLayer<Registry, opentelemetry_sdk::trace::Tracer>,
)> {
    let (provider, layer) = build_tracer_provider(&config)?;
    Ok((OtelGuard::new(provider), layer))
}

/// Internal: build tracer provider + layer without wrapping in guard.
fn build_tracer_provider(
    config: &OtelConfig,
) -> anyhow::Result<(
    SdkTracerProvider,
    OpenTelemetryLayer<Registry, opentelemetry_sdk::trace::Tracer>,
)> {
    let exporter = SpanExporter::builder()
        .with_http()
        .with_endpoint(&config.endpoint)
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build OTLP exporter: {e}"))?;

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();

    let mut builder = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(resource);

    if let Some(trace_path) = &config.trace_file {
        let file_exporter = FileTraceExporter::new(trace_path.clone())
            .map_err(|e| anyhow::anyhow!("failed to open trace file: {e}"))?;
        builder = builder.with_simple_exporter(file_exporter);
    }

    let provider = builder.build();

    let tracer = provider.tracer("rust_scraper");

    global::set_tracer_provider(provider.clone());

    let layer = tracing_opentelemetry::layer().with_tracer(tracer);

    Ok((provider, layer))
}

/// Initialize OpenTelemetry metrics with the given config.
///
/// Creates a `MeterProvider` with a `PeriodicReader` backed by the
/// OTLP HTTP metric exporter, and installs it as the global meter provider.
///
/// Also initializes tracing (tracer provider) so the guard can shut down both.
///
/// # Arguments
///
/// * `config` - OTel configuration (endpoint, service name)
///
/// # Returns
///
/// A tuple of `(MeterProvider, OtelGuard)` where:
/// - The guard must be kept alive until program exit
/// - The provider can be used to create metric instruments
#[cfg(feature = "otel-metrics")]
pub fn init_otel_metrics(
    config: OtelConfig,
) -> anyhow::Result<(
    SdkMeterProvider,
    OtelGuard,
    OpenTelemetryLayer<Registry, opentelemetry_sdk::trace::Tracer>,
)> {
    use opentelemetry_sdk::metrics::PeriodicReader;

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .build();

    let meter_provider = if std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").is_ok() || config.endpoint != "http://localhost:4318" {
        // OTLP export to Grafana Cloud or custom collector
        use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
        use std::collections::HashMap;

        let mut headers = HashMap::new();
        if let Ok(auth) = std::env::var("OTEL_EXPORTER_OTLP_HEADERS") {
            if let Some(value) = auth.strip_prefix("Authorization=") {
                headers.insert("Authorization".to_string(), value.to_string());
            }
        }

        let exporter = MetricExporter::builder()
            .with_http()
            .with_endpoint(&config.endpoint)
            .with_headers(headers)
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build OTLP metric exporter: {e}"))?;

        let reader = PeriodicReader::builder(exporter)
            .with_interval(std::time::Duration::from_secs(5))
            .build();

        SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(resource)
            .build()
    } else {
        // Console export for local development — prints metrics to stdout
        use opentelemetry_sdk::metrics::PeriodicReader;
        use opentelemetry_sdk::metrics::export::stdout::StdoutExporterBuilder;

        let exporter = StdoutExporterBuilder::new()
            .with_writer(std::io::stdout())
            .build();

        let reader = PeriodicReader::builder(exporter)
            .with_interval(std::time::Duration::from_secs(10))
            .build();

        SdkMeterProvider::builder()
            .with_reader(reader)
            .with_resource(resource)
            .build()
    };

    global::set_meter_provider(meter_provider.clone());

    let (tracer_provider, layer) = build_tracer_provider(&config)?;
    let guard = OtelGuard::new(tracer_provider).with_meter_provider(meter_provider.clone());

    Ok((meter_provider, guard, layer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_otel_config_from_env_defaults() {
        // Clear any existing env vars to test defaults
        env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        env::remove_var("OTEL_SERVICE_NAME");

        let config = OtelConfig::from_env();
        assert_eq!(config.endpoint, "http://localhost:4318");
        assert_eq!(config.service_name, "rust_scraper");
    }

    #[test]
    fn test_otel_config_from_env_custom() {
        env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://custom:9999");
        env::set_var("OTEL_SERVICE_NAME", "my-scraper");

        let config = OtelConfig::from_env();
        assert_eq!(config.endpoint, "http://custom:9999");
        assert_eq!(config.service_name, "my-scraper");

        // Clean up
        env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        env::remove_var("OTEL_SERVICE_NAME");
    }

    #[test]
    fn test_otel_config_builder_methods() {
        let config = OtelConfig::from_env()
            .with_endpoint("http://jaeger:4318")
            .with_service_name("test-scraper");

        assert_eq!(config.endpoint, "http://jaeger:4318");
        assert_eq!(config.service_name, "test-scraper");
    }

    #[test]
    fn test_otel_guard_drop_without_panic() {
        // Create a guard with no providers — should not panic on drop
        let guard = OtelGuard {
            provider: None,
            #[cfg(feature = "otel-metrics")]
            meter_provider: None,
        };
        drop(guard);
    }

    #[test]
    fn test_init_otel_tracing_bad_endpoint() {
        env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        env::remove_var("OTEL_SERVICE_NAME");

        let config = OtelConfig::from_env().with_endpoint("http://255.255.255.255:99999");
        let result = init_otel_tracing(config);
        // BatchSpanProcessor creation is lazy — init should succeed even with bad endpoint
        assert!(result.is_ok());
    }

    #[cfg(feature = "otel-metrics")]
    #[test]
    fn test_init_otel_metrics_bad_endpoint() {
        env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        env::remove_var("OTEL_SERVICE_NAME");

        let config = OtelConfig::from_env().with_endpoint("http://255.255.255.255:99999");
        let result = init_otel_metrics(config);
        // PeriodicReader creation is lazy — init should succeed even with bad endpoint
        assert!(result.is_ok());
    }

    #[cfg(feature = "otel-metrics")]
    #[test]
    fn test_init_otel_metrics_returns_guard() {
        env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
        env::remove_var("OTEL_SERVICE_NAME");

        let config = OtelConfig::from_env();
        let result = init_otel_metrics(config);
        let (_meter, guard, _layer) = result.unwrap();
        // Verify guard was created (drop should not panic)
        drop(guard);
    }
}
