//! File-based OTel span exporter — writes spans as JSONL for offline debugging.
//!
//! Enabled when `--trace-file <path>` is passed. Each line is one JSON object
//! representing a single `SpanData`. No new dependencies required — uses
//! `serde_json` (already available) and `std::fs::File`.
//!
//! # Feature Gate
//!
//! This module is only compiled when the `otel` feature is enabled.

use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use opentelemetry::trace::SpanKind;
use opentelemetry::trace::Status;
use opentelemetry::KeyValue;
use opentelemetry_sdk::trace::SpanData;
use opentelemetry_sdk::trace::SpanExporter;
use opentelemetry_sdk::Resource;
use serde_json::{json, Value};

/// JSONL file exporter for OpenTelemetry spans.
///
/// Writes one JSON line per span to the configured file path.
/// Uses `BufWriter` for efficient I/O and `Mutex` for thread-safe access
/// (export takes `&self`).
pub struct FileTraceExporter {
    writer: Mutex<BufWriter<File>>,
}

impl FileTraceExporter {
    /// Create a new file exporter, opening (or creating) the file at `path`.
    ///
    /// Parent directories are created automatically. The file is truncated
    /// on creation so each run produces a clean trace file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be created or opened.
    pub fn new(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::create(&path)?;
        let writer = BufWriter::new(file);
        Ok(Self {
            writer: Mutex::new(writer),
        })
    }
}

impl fmt::Debug for FileTraceExporter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileTraceExporter").finish_non_exhaustive()
    }
}

impl SpanExporter for FileTraceExporter {
    async fn export(&self, batch: Vec<SpanData>) -> opentelemetry_sdk::error::OTelSdkResult {
        let mut writer = self.writer.lock().map_err(|e| {
            opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("lock poisoned: {e}"))
        })?;

        for span in &batch {
            let json_line = span_to_json(span);
            let mut line = serde_json::to_vec(&json_line).map_err(|e| {
                opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("serde: {e}"))
            })?;
            line.push(b'\n');
            writer.write_all(&line).map_err(|e| {
                opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("write: {e}"))
            })?;
        }
        writer.flush().map_err(|e| {
            opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("flush: {e}"))
        })?;

        Ok(())
    }

    fn shutdown_with_timeout(&self, _timeout: Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        let mut writer = self.writer.lock().map_err(|e| {
            opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("lock poisoned: {e}"))
        })?;
        writer.flush().map_err(|e| {
            opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("flush: {e}"))
        })?;
        Ok(())
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        let mut writer = self.writer.lock().map_err(|e| {
            opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("lock poisoned: {e}"))
        })?;
        writer.flush().map_err(|e| {
            opentelemetry_sdk::error::OTelSdkError::InternalFailure(format!("flush: {e}"))
        })?;
        Ok(())
    }

    fn set_resource(&mut self, _resource: &Resource) {}
}

fn span_to_json(span: &SpanData) -> Value {
    let span_context = &span.span_context;
    let trace_id = span_context.trace_id().to_string();
    let span_id = span_context.span_id().to_string();
    let name = span.name.to_string();

    let attributes: Vec<serde_json::Value> =
        span.attributes.iter().map(attribute_to_json).collect();

    let events: Vec<serde_json::Value> = span
        .events
        .iter()
        .map(|event| {
            let attrs: Vec<serde_json::Value> =
                event.attributes.iter().map(attribute_to_json).collect();
            json!({
                "name": event.name.to_string(),
                "time_nanos": system_time_to_nanos(event.timestamp),
                "attributes": attrs,
            })
        })
        .collect();

    let links: Vec<serde_json::Value> = span
        .links
        .iter()
        .map(|link| {
            json!({
                "trace_id": link.span_context.trace_id().to_string(),
                "span_id": link.span_context.span_id().to_string(),
            })
        })
        .collect();

    json!({
        "trace_id": trace_id,
        "span_id": span_id,
        "parent_span_id": span.parent_span_id.to_string(),
        "name": name,
        "kind": span_kind_name(&span.span_kind),
        "start_time_nanos": system_time_to_nanos(span.start_time),
        "end_time_nanos": system_time_to_nanos(span.end_time),
        "status": status_name(&span.status),
        "status_message": status_message(&span.status),
        "attributes": attributes,
        "events": events,
        "links": links,
        "dropped_attributes_count": span.dropped_attributes_count,
        "dropped_events_count": span.events.dropped_count,
        "dropped_links_count": span.links.dropped_count,
        "instrumentation_scope": span.instrumentation_scope.name().to_string(),
    })
}

fn attribute_to_json(kv: &KeyValue) -> Value {
    json!({
        "key": kv.key.as_str(),
        "value": value_to_json(&kv.value),
    })
}

fn value_to_json(val: &opentelemetry::Value) -> Value {
    use opentelemetry::Value;
    match val {
        Value::Bool(b) => json!(b),
        Value::I64(i) => json!(i),
        Value::F64(f) => json!(f),
        Value::String(s) => json!(s.as_str()),
        Value::Array(arr) => {
            let items: Vec<serde_json::Value> = match arr {
                opentelemetry::Array::Bool(bools) => bools.iter().map(|b| json!(*b)).collect(),
                opentelemetry::Array::I64(ints) => ints.iter().map(|i| json!(*i)).collect(),
                opentelemetry::Array::F64(floats) => floats.iter().map(|f| json!(*f)).collect(),
                opentelemetry::Array::String(strs) => {
                    strs.iter().map(|s| json!(s.as_str())).collect()
                },
                _ => vec![],
            };
            json!(items)
        },
        _ => json!(null),
    }
}

fn system_time_to_nanos(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn span_kind_name(kind: &SpanKind) -> &'static str {
    match kind {
        SpanKind::Internal => "internal",
        SpanKind::Server => "server",
        SpanKind::Client => "client",
        SpanKind::Producer => "producer",
        SpanKind::Consumer => "consumer",
    }
}

fn status_name(status: &Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Error { .. } => "error",
        Status::Unset => "unset",
    }
}

fn status_message(status: &Status) -> &str {
    match status {
        Status::Error { description } => description.as_ref(),
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{SpanId, TraceFlags, TraceId, TraceState};
    use opentelemetry::InstrumentationScope;

    fn make_test_span(name: &str) -> SpanData {
        let span_context = opentelemetry::trace::SpanContext::new(
            TraceId::from(1u128),
            SpanId::from(1u64),
            TraceFlags::default(),
            false,
            TraceState::default(),
        );
        SpanData {
            span_context,
            parent_span_id: SpanId::INVALID,
            parent_span_is_remote: false,
            span_kind: SpanKind::Client,
            name: name.to_string().into(),
            start_time: UNIX_EPOCH,
            end_time: UNIX_EPOCH + Duration::from_millis(100),
            attributes: vec![],
            dropped_attributes_count: 0,
            events: opentelemetry_sdk::trace::SpanEvents::default(),
            links: opentelemetry_sdk::trace::SpanLinks::default(),
            status: Status::Ok,
            instrumentation_scope: InstrumentationScope::builder("test").build(),
        }
    }

    #[test]
    fn test_file_exporter_writes_jsonl() {
        let dir = std::env::temp_dir().join("rust_scraper_test_trace");
        let path = dir.join("test_trace.jsonl");

        let exporter = FileTraceExporter::new(path.clone()).expect("create exporter");
        let spans = vec![make_test_span("test-span")];

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(exporter.export(spans)).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 1);

        let parsed: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed["name"], "test-span");
        assert_eq!(parsed["kind"], "client");
        assert_eq!(parsed["status"], "ok");

        // Clean up
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn test_span_to_json_attributes() {
        let mut span = make_test_span("with-attrs");
        span.attributes = vec![KeyValue::new("http.method", "GET")];

        let json = span_to_json(&span);
        let attrs = json["attributes"].as_array().unwrap();
        assert_eq!(attrs.len(), 1);
        assert_eq!(attrs[0]["key"], "http.method");
        assert_eq!(attrs[0]["value"], "GET");
    }

    #[test]
    fn test_status_message_error() {
        let mut span = make_test_span("err-span");
        span.status = Status::Error {
            description: "test error".into(),
        };
        let json = span_to_json(&span);
        assert_eq!(json["status"], "error");
    }
}
