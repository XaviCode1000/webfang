//! File-based tracing layer — writes JSONL trace files without OTel dependency.
//!
//! Enabled when `--trace-file <path>` is passed. Each line is one JSON object
//! representing a tracing event or span. Replaces the OTel-coupled
//! `FileTraceExporter` with a native `tracing_subscriber::Layer`.
//!
//! The file is **truncated** on creation — each run produces a clean trace file.
//! Structured fields from `tracing::info!(key = value, ...)` are captured in the
//! `fields` object. `trace_id` is a **logical** identifier: when the `otel`
//! feature is active it uses the real W3C trace ID from the OpenTelemetry span
//! context; otherwise it falls back to the root span's ID (stable across worker
//! threads) and finally to a stable per-invocation seed. This replaces the old
//! thread-ID-based `trace_id`, which fragmented across threads. `parent_id` is
//! also emitted so the logical trace tree is reconstructable from the JSONL.
//!
//! **Thread-safety note:** This layer uses thread-local span tracking
//! (`SPAN_STACK`). It assumes `on_enter`/`on_exit`/`on_event` are called from
//! the same thread for a given span lifecycle — guaranteed by
//! `tracing_subscriber::Registry`.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::SystemTime;

use serde_json::{json, Map, Value};
use tracing::Subscriber;
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

// Thread-local span stack for tracking the current span inside `on_event`.
// When a span is entered, its ID is pushed; when exited, it's popped IF the
// exiting ID matches the top. This prevents stack corruption from out-of-order
// span exits.
thread_local! {
    static SPAN_STACK: RefCell<Vec<tracing::Id>> = const { RefCell::new(Vec::new()) };
}

/// A `tracing_subscriber::Layer` that writes JSONL trace files.
///
/// Each line is a JSON object with: `timestamp` (RFC3339), `level`, `target`,
/// `span` (name, when inside a span), `trace_id`, `span_id`, `message`,
/// and `fields` (all structured key-value pairs from the event).
pub struct FileTraceLayer {
    writer: Mutex<BufWriter<File>>,
    /// Stable per-invocation seed used as the fallback `trace_id` when no
    /// logical span context is available (e.g. events outside any span).
    trace_id_seed: u64,
}

impl FileTraceLayer {
    /// Create a new file trace layer, opening (or creating) the file at `path`.
    ///
    /// Parent directories are created automatically. The file is truncated on
    /// creation so each run produces a clean trace file.
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
            trace_id_seed: make_trace_seed(),
        })
    }
}

impl std::fmt::Debug for FileTraceLayer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileTraceLayer").finish_non_exhaustive()
    }
}

impl Drop for FileTraceLayer {
    fn drop(&mut self) {
        if let Ok(writer) = self.writer.get_mut() {
            if let Err(e) = writer.flush() {
                eprintln!("[FileTraceLayer] flush on drop failed: {e}");
            }
        }
    }
}

impl<S> Layer<S> for FileTraceLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_enter(&self, id: &tracing::Id, _ctx: Context<'_, S>) {
        SPAN_STACK.with(|stack| stack.borrow_mut().push(id.clone()));
    }

    fn on_exit(&self, id: &tracing::Id, _ctx: Context<'_, S>) {
        SPAN_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            // Only pop if the exiting ID matches the top of the stack.
            // Out-of-order exits (e.g., inner guard dropped before outer)
            // would corrupt the stack — this check prevents that.
            if stack.last() == Some(id) {
                stack.pop();
            }
        });
    }

    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        id: &tracing::Id,
        ctx: Context<'_, S>,
    ) {
        // Capture the span's structured fields at creation time so they can be
        // serialized into the JSONL later (S2 offline visibility).
        let mut recorder = EventRecorder::new();
        attrs.record(&mut recorder);
        if let Some(span_ref) = ctx.span(id) {
            span_ref.extensions_mut().insert(recorder);
        }
    }

    fn on_event(&self, event: &tracing::Event<'_>, ctx: Context<'_, S>) {
        let meta = event.metadata();

        let mut record = json!({
            "timestamp": system_time_to_rfc3339(SystemTime::now()),
            "level": meta.level().as_str(),
            "target": meta.target(),
        });

        // Span context from thread-local span stack
        let current_span_id = SPAN_STACK.with(|stack| stack.borrow().last().cloned());

        if let Some(ref id) = current_span_id {
            if let Some(span_ref) = ctx.span(id) {
                record["span"] = json!(span_ref.name());
                record["span_id"] = json!(format!("{:016x}", id.into_u64()));

                // Capture span fields from the CURRENT span AND its parents (the
                // scope), so the root span's fields (e.g. the seed URL) become
                // visible offline without an external OTel collector (S2).
                // Child fields take precedence on key collision.
                let mut span_fields: Map<String, Value> = Map::new();
                for ancestor in span_ref.scope() {
                    if let Some(rec) = ancestor.extensions().get::<EventRecorder>() {
                        for (k, v) in &rec.fields {
                            span_fields.entry(k.clone()).or_insert(v.clone());
                        }
                    }
                }
                if !span_fields.is_empty() {
                    record["span_fields"] = Value::Object(span_fields);
                }
            }
        }

        // parent_id: reconstruct the logical trace tree. The current span's
        // parent (enclosing span) is serialized so the tree is recoverable from
        // the JSONL. Previously absent -> correlation was impossible (D2).
        if let Some(current) = ctx.lookup_current() {
            if let Some(parent) = current.parent() {
                record["parent_id"] = json!(format!("{:016x}", parent.id().into_u64()));
            }
        }

        // trace_id: logical identifier that survives thread hops (D3).
        // 1) OTel W3C trace ID when `otel` is active and a valid OTel context
        //    exists; 2) the root span's ID (stable for the whole run, survives
        //    worker-thread hops); 3) a stable per-invocation seed fallback.
        #[cfg(feature = "otel")]
        let trace_id =
            otel_trace_id().unwrap_or_else(|| logical_trace_id(self.trace_id_seed, &ctx));
        #[cfg(not(feature = "otel"))]
        let trace_id = logical_trace_id(self.trace_id_seed, &ctx);
        record["trace_id"] = json!(trace_id);

        // Single-pass field capture: extracts all fields AND the message
        // in one traversal, avoiding the double-visit antipattern.
        let mut recorder = EventRecorder::new();
        event.record(&mut recorder);

        if !recorder.fields.is_empty() {
            record["fields"] = Value::Object(recorder.fields);
        }
        if let Some(m) = recorder.message {
            record["message"] = json!(m);
        }

        if let Ok(mut writer) = self.writer.lock() {
            let mut line = match serde_json::to_vec(&record) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[FileTraceLayer] serialization error: {e}");
                    return;
                },
            };
            line.push(b'\n');
            if let Err(e) = writer.write_all(&line) {
                eprintln!("[FileTraceLayer] write error: {e}");
            }
            if let Err(e) = writer.flush() {
                eprintln!("[FileTraceLayer] flush error: {e}");
            }
        }
    }
}

/// Resolve a logical `trace_id` independent of the OS thread, so it stays
/// stable when a task hops between worker threads (D3). Prefers the root
/// span's ID; falls back to a per-invocation seed when no span is current.
fn logical_trace_id<S>(seed: u64, ctx: &Context<'_, S>) -> String
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    if let Some(current) = ctx.lookup_current() {
        if let Some(root) = current.scope().last() {
            return format!("{:016x}", root.id().into_u64());
        }
    }
    format!("{:016x}", seed)
}

/// Build a stable per-invocation seed used as the fallback `trace_id` when no
/// logical span context is available.
fn make_trace_seed() -> u64 {
    use std::hash::{Hash, Hasher};
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    nanos.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    hasher.finish()
}

/// Extract the real W3C trace ID from the OpenTelemetry span context, if the
/// `otel` feature is active and a valid OTel context is present.
#[cfg(feature = "otel")]
fn otel_trace_id() -> Option<String> {
    use opentelemetry::trace::TraceContextExt;
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    let cx = tracing::Span::current().context();
    let span_ref = cx.span();
    let sc = span_ref.span_context();
    if sc.is_valid() {
        return Some(format!("{:032x}", sc.trace_id()));
    }
    None
}

/// Single-pass event recorder. Captures ALL fields (including `message`) in one
/// traversal of the event's field set, avoiding the double-visit antipattern.
#[derive(Clone)]
struct EventRecorder {
    fields: Map<String, Value>,
    message: Option<String>,
}

impl EventRecorder {
    fn new() -> Self {
        Self {
            fields: Map::new(),
            message: None,
        }
    }
}

impl tracing::field::Visit for EventRecorder {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let name = field.name().to_string();
        let formatted = format!("{value:?}");

        if name == "message" {
            let msg =
                if formatted.starts_with('"') && formatted.ends_with('"') && formatted.len() >= 2 {
                    formatted[1..formatted.len() - 1].to_string()
                } else {
                    formatted
                };
            self.message = Some(msg);
        } else {
            let value =
                if formatted.starts_with('"') && formatted.ends_with('"') && formatted.len() >= 2 {
                    Value::String(formatted[1..formatted.len() - 1].to_string())
                } else {
                    Value::String(formatted)
                };
            self.fields.insert(name, value);
        }
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), Value::Bool(value));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields
                .insert(field.name().to_string(), Value::String(value.to_string()));
        }
    }
}

/// Convert a `SystemTime` to an RFC 3339 string with millisecond precision.
///
/// Format: `2025-07-09T20:41:34.123Z`
fn system_time_to_rfc3339(t: SystemTime) -> String {
    let duration = t.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Days since 1970-01-00 (Howard Hinnant algorithm)
    let z = days as i64 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if m <= 2 { y + 1 } else { y };

    format!("{yr:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::path::Path;
    use tracing_subscriber::layer::SubscriberExt;

    // Each test creates its own Dispatch to avoid cross-test interference.

    #[test]
    fn contract_creates_file_at_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let _layer = FileTraceLayer::new(path.clone()).unwrap();
        assert!(
            path.exists(),
            "trace file must be created at the given path"
        );
    }

    #[test]
    fn contract_creates_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("trace.jsonl");
        let _layer = FileTraceLayer::new(path.clone()).unwrap();
        assert!(path.exists(), "parent directories must be created");
    }

    #[test]
    fn contract_emits_valid_jsonl_per_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("hello");
        });

        let lines = read_jsonl_lines(&path);
        assert_eq!(lines.len(), 1, "should have exactly one JSONL line");
        let parsed: Value = serde_json::from_str(&lines[0]).unwrap();
        assert!(parsed.is_object(), "line must be a JSON object");
    }

    #[test]
    fn contract_timestamp_is_rfc3339() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("check timestamp");
        });

        let parsed = parse_single_event(&path);
        let ts = parsed["timestamp"]
            .as_str()
            .expect("timestamp must be a string");
        assert!(ts.ends_with('Z'), "timestamp must end with Z, got: {ts}");
        assert!(
            ts.contains('T'),
            "timestamp must contain T separator, got: {ts}"
        );
        assert!(
            ts.len() >= 20,
            "timestamp must be full ISO format, got: {ts}"
        );
        let parts: Vec<&str> = ts.split('T').collect();
        assert_eq!(parts.len(), 2, "must have date and time separated by T");
        let date_parts: Vec<&str> = parts[0].split('-').collect();
        assert_eq!(date_parts.len(), 3, "date must be YYYY-MM-DD");
        assert_eq!(date_parts[0].len(), 4, "year must be 4 digits");
    }

    #[test]
    fn contract_emits_level_target_message() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::warn!(target: "my_target", "oops");
        });

        let parsed = parse_single_event(&path);
        assert_eq!(parsed["level"], "WARN");
        assert_eq!(parsed["target"], "my_target");
        assert_eq!(parsed["message"], "oops");
    }

    #[test]
    fn contract_emits_trace_id_and_span_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            let span = tracing::info_span!("test_span");
            let _enter = span.enter();
            tracing::info!("with ids");
        });

        let parsed = parse_single_event(&path);
        let trace_id = parsed["trace_id"]
            .as_str()
            .expect("trace_id must be present");
        assert_eq!(trace_id.len(), 16, "trace_id must be 16 hex chars");
        assert!(
            parsed["span_id"].as_str().is_some(),
            "span_id must be present inside a span, got: {:?}",
            parsed["span_id"]
        );
        let span_id = parsed["span_id"].as_str().unwrap();
        assert_eq!(span_id.len(), 16, "span_id must be 16 hex chars");
    }

    #[test]
    fn contract_emits_structured_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!(request_id = 42, user = "alice", "processing");
        });

        let parsed = parse_single_event(&path);
        let fields = parsed["fields"]
            .as_object()
            .expect("fields must be a JSON object");
        assert_eq!(fields["request_id"], json!(42));
        assert_eq!(fields["user"], json!("alice"));
    }

    #[test]
    fn contract_message_not_duplicated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("unique_msg_12345");
        });

        let parsed = parse_single_event(&path);
        assert_eq!(parsed["message"], "unique_msg_12345");
        if let Some(fields) = parsed["fields"].as_object() {
            assert!(
                !fields.contains_key("message"),
                "message must not be duplicated in fields"
            );
        }
    }

    #[test]
    fn contract_span_field_present_inside_span() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            let span = tracing::info_span!("my_op", request_id = 99);
            let _enter = span.enter();
            tracing::info!("inside");
        });

        let parsed = parse_single_event(&path);
        assert_eq!(
            parsed["span"].as_str(),
            Some("my_op"),
            "span name must be set inside a span"
        );
        assert!(
            parsed["span_id"].is_string(),
            "span_id must be present inside a span"
        );
    }

    #[test]
    fn contract_captures_span_fields_from_scope() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            // Root span carries the seed URL — the key field for offline
            // telemetry (S2). A nested child adds its own field.
            let root = tracing::info_span!("root_span", seed_url = "https://example.com");
            let _rg = root.enter();
            let child = tracing::info_span!("child_span", depth = 1);
            let _cg = child.enter();
            tracing::info!("inside child");
        });

        let parsed = parse_single_event(&path);
        let span_fields = parsed["span_fields"]
            .as_object()
            .expect("span_fields must be present when a span has fields");
        assert_eq!(
            span_fields["seed_url"],
            json!("https://example.com"),
            "root span seed_url must be captured via the span scope"
        );
        assert_eq!(
            span_fields["depth"],
            json!(1),
            "child span field must also be captured"
        );
    }

    #[test]
    fn contract_span_field_absent_outside_span() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("no span");
        });

        let parsed = parse_single_event(&path);
        assert!(
            parsed["span"].is_null(),
            "span must be null when not inside a span, got: {:?}",
            parsed["span"]
        );
    }

    #[test]
    fn contract_nested_spans_track_correctly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            let outer = tracing::info_span!("outer");
            let _og = outer.enter();
            tracing::info!("in outer");

            let inner = tracing::info_span!("inner");
            let _ig = inner.enter();
            tracing::info!("in inner");
            drop(_ig);

            tracing::info!("back to outer");
        });

        let parsed = parse_lines(&path);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0]["span"], "outer");
        assert_eq!(parsed[1]["span"], "inner");
        assert_eq!(parsed[2]["span"], "outer");
    }

    #[test]
    fn contract_multiple_events_append_sequentially() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("first");
            tracing::debug!("second");
            tracing::error!("third");
        });

        let parsed = parse_lines(&path);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0]["message"], "first");
        assert_eq!(parsed[0]["level"], "INFO");
        assert_eq!(parsed[1]["message"], "second");
        assert_eq!(parsed[1]["level"], "DEBUG");
        assert_eq!(parsed[2]["message"], "third");
        assert_eq!(parsed[2]["level"], "ERROR");
    }

    #[test]
    fn contract_thread_safety_concurrent_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        let mut handles = vec![];
        for i in 0..10 {
            let d = dispatch.clone();
            handles.push(std::thread::spawn(move || {
                tracing::dispatcher::with_default(&d, || {
                    tracing::info!(thread_id = i, "msg from thread {i}");
                });
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let lines = read_jsonl_lines(&path);
        assert_eq!(lines.len(), 10, "must have 10 lines from 10 threads");

        for (i, line) in lines.iter().enumerate() {
            let parsed: Value = serde_json::from_str(line).unwrap();
            assert!(parsed.is_object(), "line {i} must be a JSON object");
            let fields = parsed["fields"]
                .as_object()
                .expect("fields must be present");
            assert!(
                fields.contains_key("thread_id"),
                "thread_id field must be captured, missing in line {i}"
            );
        }
    }

    #[test]
    fn contract_truncates_on_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        std::fs::write(&path, "old data\n").unwrap();

        let _layer = FileTraceLayer::new(path.clone()).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.is_empty(),
            "file must be truncated on creation, got: {content:?}"
        );
    }

    #[test]
    fn contract_drop_flushes_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let layer = FileTraceLayer::new(path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        let dispatch = tracing::Dispatch::new(subscriber);

        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("buffered event");
        });

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.is_empty(), "Drop must flush buffered data to disk");
    }

    fn read_jsonl_lines(path: &Path) -> Vec<String> {
        let content = std::fs::read_to_string(path).unwrap();
        content
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect()
    }

    fn parse_single_event(path: &Path) -> Value {
        let lines = read_jsonl_lines(path);
        assert_eq!(lines.len(), 1, "expected exactly 1 JSONL line");
        serde_json::from_str(&lines[0]).unwrap()
    }

    fn parse_lines(path: &Path) -> Vec<Value> {
        read_jsonl_lines(path)
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}
