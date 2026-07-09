//! File-based tracing layer — writes JSONL trace files without OTel dependency.
//!
//! Enabled when `--trace-file <path>` is passed. Each line is one JSON object
//! representing a tracing event or span. Replaces the OTel-coupled
//! `FileTraceExporter` with a native `tracing_subscriber::Layer`.
//!
//! The file is **truncated** on creation — each run produces a clean trace file.
//! Structured fields from `tracing::info!(key = value, ...)` are captured in the
//! `fields` object. `trace_id` uses the thread ID (stable within a thread); when
//! OTel is also active, the OTel trace/span IDs take precedence in downstream
//! consumers.

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
// When a span is entered, its ID is pushed; when exited, it's popped.
// This is necessary because `Span::current()` doesn't reliably return the
// span ID inside a Layer's `on_event` callback.
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

    fn on_exit(&self, _id: &tracing::Id, _ctx: Context<'_, S>) {
        SPAN_STACK.with(|stack| stack.borrow_mut().pop());
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
            }
        }

        // trace_id: use thread ID as a stable per-thread trace identifier.
        let tid_debug = format!("{:?}", std::thread::current().id());
        let tid_num: u64 = tid_debug
            .trim_start_matches("ThreadId(")
            .trim_end_matches(')')
            .parse()
            .unwrap_or(0);
        record["trace_id"] = json!(format!("{tid_num:016x}"));

        // Capture all structured fields from the event
        let fields = event_all_fields(event);
        if !fields.is_empty() {
            record["fields"] = Value::Object(fields);
        }

        // Extract message separately for top-level convenience
        let msg = event_message(event);
        if let Some(m) = msg {
            record["message"] = json!(m);
        }

        if let Ok(mut writer) = self.writer.lock() {
            let mut line = match serde_json::to_vec(&record) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("[FileTraceLayer] serialization error: {e}");
                    return;
                }
            };
            line.push(b'\n');
            if let Err(e) = writer.write_all(&line) {
                eprintln!("[FileTraceLayer] write error: {e}");
            }
            // Flush on every event to ensure data reaches disk.
            // This is intentional for a tracing layer — data loss on crash
            // is worse than the syscall cost. The BufWriter still batches
            // within a single lock acquisition.
            if let Err(e) = writer.flush() {
                eprintln!("[FileTraceLayer] flush error: {e}");
            }
        }
    }
}

/// Extract all fields from a tracing event as a JSON map.
fn event_all_fields(event: &tracing::Event<'_>) -> Map<String, Value> {
    let mut recorder = AllFieldsRecorder::new();
    event.record(&mut recorder);
    recorder.fields
}

/// Extract just the `message` field from a tracing event.
fn event_message(event: &tracing::Event<'_>) -> Option<String> {
    let mut recorder = MessageRecorder(String::new());
    event.record(&mut recorder);
    if recorder.0.is_empty() {
        None
    } else {
        Some(recorder.0)
    }
}

/// Captures all fields from a tracing event.
struct AllFieldsRecorder {
    fields: Map<String, Value>,
}

impl AllFieldsRecorder {
    fn new() -> Self {
        Self {
            fields: Map::new(),
        }
    }
}

impl tracing::field::Visit for AllFieldsRecorder {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let name = field.name().to_string();
        let formatted = format!("{value:?}");
        // Strip surrounding quotes from Debug output on strings
        let value = if formatted.starts_with('"') && formatted.ends_with('"') && formatted.len() >= 2
        {
            Value::String(formatted[1..formatted.len() - 1].to_string())
        } else {
            Value::String(formatted)
        };
        self.fields.insert(name, value);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields
            .insert(field.name().to_string(), Value::Number(value.into()));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if let Some(n) = serde_json::Number::from_u128(value as u128) {
            self.fields
                .insert(field.name().to_string(), Value::Number(n));
        }
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), Value::Bool(value));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .insert(field.name().to_string(), Value::String(value.to_string()));
    }
}

/// Captures only the `message` field.
struct MessageRecorder(String);

impl tracing::field::Visit for MessageRecorder {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
            // Strip surrounding quotes from Debug output
            if self.0.starts_with('"') && self.0.ends_with('"') && self.0.len() >= 2 {
                self.0 = self.0[1..self.0.len() - 1].to_string();
            }
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

    // Convert seconds since epoch to Y-M-D H:M:S using integer arithmetic
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;

    // Days since 1970-01-00 (algorithm from Howard Hinnant)
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

    format!(
        "{yr:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use tracing_subscriber::layer::SubscriberExt;

    // Each test creates its own Dispatch to avoid cross-test interference
    // from set_global_default (which only works once per process) or
    // set_default (which is thread-local and can leak across tests).

    // ── Contract-based tests: each test validates ONE observable behavior ──

    #[test]
    fn contract_creates_file_at_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trace.jsonl");
        let _layer = FileTraceLayer::new(path.clone()).unwrap();
        assert!(path.exists(), "trace file must be created at the given path");
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
        let ts = parsed["timestamp"].as_str().expect("timestamp must be a string");
        // RFC3339: 2025-07-09T20:41:34.123Z
        assert!(ts.ends_with('Z'), "timestamp must end with Z, got: {ts}");
        assert!(ts.contains('T'), "timestamp must contain T separator, got: {ts}");
        assert!(ts.len() >= 20, "timestamp must be full ISO format, got: {ts}");
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
        // span_id is only present when inside a span
        if let Some(span_id) = parsed["span_id"].as_str() {
            assert_eq!(span_id.len(), 16, "span_id must be 16 hex chars");
        }
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

    // ── Helpers ──

    fn read_jsonl_lines(path: &PathBuf) -> Vec<String> {
        let content = std::fs::read_to_string(path).unwrap();
        content
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect()
    }

    fn parse_single_event(path: &PathBuf) -> Value {
        let lines = read_jsonl_lines(path);
        assert_eq!(lines.len(), 1, "expected exactly 1 JSONL line");
        serde_json::from_str(&lines[0]).unwrap()
    }

    fn parse_lines(path: &PathBuf) -> Vec<Value> {
        read_jsonl_lines(path)
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}
