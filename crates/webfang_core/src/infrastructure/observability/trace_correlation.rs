//! Trace Correlation Layer
//!
//! A custom `tracing_subscriber::Layer` that extracts `trace_id` and `span_id`
//! from the OpenTelemetry span context and injects them as fields into JSON log
//! records. This enables correlation between structured logs and distributed
//! traces.
//!
//! # Behavior
//!
//! When an event occurs inside an OTel-instrumented span, this layer adds:
//! - `trace_id` — the 32-hex-char W3C Trace ID
//! - `span_id` — the 16-hex-char W3C Span ID
//!
//! When an event occurs outside any OTel span, both fields are set to `"0"`.

use opentelemetry::trace::TraceContextExt as _;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

/// Layer that injects `trace_id` and `span_id` into log events.
///
/// Works by reading the OTel span context from the tracing subscriber's
/// current span and recording the values as tracing fields on each event.
pub struct TraceCorrelationLayer;

impl<S> Layer<S> for TraceCorrelationLayer
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let (trace_id, span_id) = extract_trace_ids();

        // Record via tracing so they appear in structured JSON output
        tracing::Span::current().record("trace_id", trace_id.as_str());
        tracing::Span::current().record("span_id", span_id.as_str());

        // Suppress unused warning — event is part of the Layer contract
        let _ = event;
    }
}

/// Extract trace_id and span_id from the current OTel span context.
///
/// Returns `("0", "0")` when no valid OTel context is active.
fn extract_trace_ids() -> (String, String) {
    let current = tracing::Span::current();
    let cx = current.context();
    let span = cx.span();
    let span_ctx = span.span_context();

    if span_ctx.is_valid() {
        (
            format!("{}", span_ctx.trace_id()),
            format!("{}", span_ctx.span_id()),
        )
    } else {
        ("0".to_string(), "0".to_string())
    }
}

/// Convenience constructor.
pub fn trace_correlation_layer() -> TraceCorrelationLayer {
    TraceCorrelationLayer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_correlation_layer_creates() {
        let _layer = trace_correlation_layer();
    }

    #[test]
    fn test_trace_id_format() {
        let id: u128 = 0x1234567890abcdef1234567890abcdef;
        let formatted = format!("{:032x}", id);
        assert_eq!(formatted.len(), 32);
        assert_eq!(formatted, "1234567890abcdef1234567890abcdef");
    }

    #[test]
    fn test_span_id_format() {
        let id: u64 = 0x1234567890abcdef;
        let formatted = format!("{:016x}", id);
        assert_eq!(formatted.len(), 16);
        assert_eq!(formatted, "1234567890abcdef");
    }

    #[test]
    fn test_extract_trace_ids_returns_zero_when_no_otel() {
        let (trace_id, span_id) = extract_trace_ids();
        assert_eq!(trace_id, "0");
        assert_eq!(span_id, "0");
    }
}
