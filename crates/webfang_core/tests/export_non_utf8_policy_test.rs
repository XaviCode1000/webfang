//! Out-of-crate proof of the #1812 non-UTF-8 export policy.
//!
//! The DECIDED policy: a JSONL line that is not valid UTF-8 is SKIPPED, the
//! export COMPLETES, the records around it survive untouched, and the skip is
//! REPORTED — the count of skipped lines and the kind of the damage reach
//! the operator as structured fields.
//!
//! This file is the EXTERNAL proof: it drives the public export ports
//! (`process_results`, `process_results_with_chunks`) and asserts on the
//! observable bytes of the export file, on the persisted records, and on the
//! emitted `tracing` events.
//!
//! # Why a local capture layer
//!
//! The in-crate capture helper
//! (`infrastructure::observability::test_log_capture::LogBuffer`) is
//! `#[cfg(test)]`, so it does NOT exist in the library an integration test
//! links against. This file therefore installs its own small `Layer` as the
//! thread-local default subscriber and records the fields it asserts on —
//! the message, the stage, the error kind and the skipped count.
//!
//! A `fmt::TestWriter` would NOT do: it snapshots the buffer at install
//! time, so the assertions would read an empty log and the test would fail
//! for the wrong reason.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use webfang_core::application::export_factory::{
    process_results, process_results_with_chunks, ResumeContext,
};
use webfang_core::domain::entities::ExportFormat;
use webfang_core::domain::page_state::PageStatus;
use webfang_core::domain::{DocumentChunkUnvalidated, ScrapedContent, ValidUrl};
use webfang_core::infrastructure::export::RecordStore;

// =========================================================================
// Event capture
// =========================================================================

/// One captured `tracing` event: its message and its fields, flattened to
/// strings so the assertions do not depend on the value's Rust type.
#[derive(Clone, Debug)]
struct CapturedEvent {
    message: String,
    fields: BTreeMap<String, String>,
}

/// Flattens an event's fields into `name -> rendered value` pairs.
///
/// The event's message is the field named `message` (that is where `tracing`
/// keeps it), so it is lifted out of the map instead of being asserted
/// through it.
#[derive(Default)]
struct FieldVisitor {
    message: Option<String>,
    fields: BTreeMap<String, String>,
}

impl FieldVisitor {
    /// A `Debug`-rendered value may arrive quoted (`"text"`); the quotes are
    /// not part of the value.
    fn unquote(rendered: String) -> String {
        if rendered.len() >= 2 && rendered.starts_with('"') && rendered.ends_with('"') {
            rendered[1..rendered.len() - 1].to_string()
        } else {
            rendered
        }
    }

    fn insert(&mut self, field: &Field, rendered: String) {
        if field.name() == "message" {
            self.message = Some(Self::unquote(rendered));
        } else {
            self.fields.insert(field.name().to_string(), rendered);
        }
    }
}

impl Visit for FieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.insert(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.insert(field, Self::unquote(format!("{value:?}")));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.insert(field, value.to_string());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.insert(field, value.to_string());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.insert(field, value.to_string());
    }
}

/// The `Layer` that captures every event the run emits.
#[derive(Clone, Default)]
struct CaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        if let Ok(mut events) = self.events.lock() {
            events.push(CapturedEvent {
                message: visitor.message.unwrap_or_default(),
                fields: visitor.fields,
            });
        }
    }
}

/// Run `body` with the capture layer installed as the thread-local default
/// subscriber and return the captured events.
fn capture(body: impl FnOnce()) -> Vec<CapturedEvent> {
    let layer = CaptureLayer::default();
    let subscriber = tracing_subscriber::registry().with(layer.clone());
    tracing::subscriber::with_default(subscriber, body);
    layer
        .events
        .lock()
        .map(|events| events.clone())
        .unwrap_or_default()
}

/// Every `skipped_non_utf8` value the run reported.
fn skipped_non_utf8_values(events: &[CapturedEvent]) -> Vec<u64> {
    events
        .iter()
        .filter_map(|event| event.fields.get("skipped_non_utf8"))
        .filter_map(|value| value.parse().ok())
        .collect()
}

/// The structured error event that reports the skipped lines.
fn skip_error_event(events: &[CapturedEvent]) -> Option<&CapturedEvent> {
    events
        .iter()
        .find(|event| event.fields.get("stage").map(String::as_str) == Some("jsonl_resume_index"))
}

// =========================================================================
// Fixtures
// =========================================================================

/// Bytes that are not valid UTF-8 — the same input class the config
/// loader's contract test uses (`[0xff, 0xfe, 0x00]`).
const INVALID_UTF8_LINE: [u8; 3] = [0xff, 0xfe, 0x00];

/// The number of damaged lines the fixture carries.
const DAMAGED_LINES: u64 = 2;

fn scraped(url: &str, title: &str, body: &str) -> ScrapedContent {
    ScrapedContent {
        title: title.to_string(),
        content: body.to_string(),
        url: ValidUrl::parse(url).expect("valid url"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    }
}

/// One valid export line.
fn valid_line(url: &str, checksum: &str) -> String {
    format!("{{\"url\":\"{url}\",\"content\":\"kept\",\"checksum_sha256\":\"{checksum}\"}}\n")
}

/// An export file that holds, in order: a valid record, TWO damaged lines,
/// and a valid record AFTER the damage. Returns the path and the exact
/// seeded bytes.
fn write_damaged_export(dir: &std::path::Path) -> (std::path::PathBuf, Vec<u8>) {
    let path = dir.join("export.jsonl");
    let mut bytes = valid_line("https://kept.test/a", "aaaa").into_bytes();
    bytes.extend_from_slice(&INVALID_UTF8_LINE);
    bytes.push(b'\n');
    bytes.extend_from_slice(&INVALID_UTF8_LINE);
    bytes.push(b'\n');
    bytes.extend_from_slice(valid_line("https://tail.test/c", "cccc").as_bytes());
    std::fs::write(&path, &bytes).expect("damaged export file");
    (path, bytes)
}

// =========================================================================
// 1. The count is accurate and the export COMPLETES (both paths)
// =========================================================================

/// The plain path: the count of skipped lines equals the number of damaged
/// lines, and the export COMPLETES.
#[test]
fn the_plain_path_counts_every_damaged_line_and_completes() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let (_path, _seeded) = write_damaged_export(dir.path());

    let events = capture(|| {
        process_results(
            &[scraped("https://new.test/b", "B", "body of the new record")],
            dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
        )
        .expect("the export must COMPLETE even with damaged lines");
    });

    let counts = skipped_non_utf8_values(&events);
    assert!(
        !counts.is_empty(),
        "the run must report the skipped lines: {events:?}"
    );
    assert!(
        counts.iter().all(|count| *count == DAMAGED_LINES),
        "every reported count must equal the number of damaged lines: {counts:?}"
    );
}

/// The `--clean-ai` path: the same policy, the same count.
#[test]
fn the_clean_ai_path_counts_every_damaged_line_and_completes() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let (_path, _seeded) = write_damaged_export(dir.path());

    let chunk = DocumentChunkUnvalidated::new(
        uuid::Uuid::new_v4(),
        "https://new.test/d",
        "D",
        "body of the cleaned chunk",
    );

    let events = capture(|| {
        process_results_with_chunks(
            &[chunk],
            dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
        )
        .expect("the --clean-ai export must COMPLETE even with damaged lines");
    });

    let counts = skipped_non_utf8_values(&events);
    assert!(
        !counts.is_empty(),
        "the run must report the skipped lines: {events:?}"
    );
    assert!(
        counts.iter().all(|count| *count == DAMAGED_LINES),
        "every reported count must equal the number of damaged lines: {counts:?}"
    );
}

/// The damage is reported with its KIND: `io::Error`'s `Display` omits the
/// `ErrorKind`, so the event must carry it as a structured field.
#[test]
fn the_skipped_lines_are_reported_with_their_error_kind() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let (_path, _seeded) = write_damaged_export(dir.path());

    let events = capture(|| {
        process_results(
            &[scraped("https://new.test/b", "B", "body of the new record")],
            dir.path().to_path_buf(),
            ExportFormat::Jsonl,
            "export",
            None,
        )
        .expect("the export must COMPLETE even with damaged lines");
    });

    let event = skip_error_event(&events).expect("the skip must be reported");
    assert_eq!(
        event.fields.get("error_kind").map(String::as_str),
        Some("InvalidData"),
        "the event must carry the ErrorKind: {event:?}"
    );
    assert!(
        event.message.contains("not valid UTF-8"),
        "the message must name the damage: {event:?}"
    );
}

// =========================================================================
// 2. The records around the damage survive; the file only grows
// =========================================================================

/// The records BEFORE and AFTER the damaged lines survive, and the
/// pre-existing bytes are preserved verbatim: the file only grows.
#[test]
fn the_records_around_the_damage_survive_and_the_file_only_grows() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let (path, seeded) = write_damaged_export(dir.path());

    process_results(
        &[scraped("https://new.test/b", "B", "body of the new record")],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        None,
    )
    .expect("the export must COMPLETE even with damaged lines");

    let after = std::fs::read(&path).expect("export file");
    assert!(
        after.len() > seeded.len(),
        "the export must only append: {} bytes -> {} bytes",
        seeded.len(),
        after.len()
    );
    assert_eq!(
        &after[..seeded.len()],
        &seeded[..],
        "the pre-existing bytes must be preserved verbatim"
    );

    let body = String::from_utf8_lossy(&after);
    assert!(
        body.contains("kept.test/a"),
        "the record BEFORE the damage must survive: {body}"
    );
    assert!(
        body.contains("tail.test/c"),
        "the record AFTER the damage must survive: {body}"
    );
    assert!(
        body.contains("new.test/b"),
        "the new record must be appended: {body}"
    );
}

// =========================================================================
// 3. An AI chunk that fails validation is NOT committed
// =========================================================================

/// A chunk that cannot be validated never reaches the file, so it must not
/// be committed either: a committed record that was never written makes the
/// resume gate skip the page forever.
#[test]
fn an_ai_chunk_that_fails_validation_is_not_committed_and_its_failure_is_recorded() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let state_dir = tempfile::TempDir::new().expect("state dir");
    let store = RecordStore::new("export-non-utf8-policy.test")
        .with_state_dir(state_dir.path().to_path_buf());
    let ctx_store = store.clone();
    let ctx = ResumeContext::new(&ctx_store).with_resume(false);

    // An empty title fails `validate()` — the chunk never reaches the file.
    let chunk = DocumentChunkUnvalidated::new(
        uuid::Uuid::new_v4(),
        "https://invalid.test/a",
        "",
        "the body of a chunk that cannot be validated",
    );

    process_results_with_chunks(
        &[chunk],
        dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        Some(&ctx),
    )
    .expect("the export must COMPLETE");

    let path = dir.path().join("export.jsonl");
    let written = std::fs::read(&path).unwrap_or_default();
    let body = String::from_utf8_lossy(&written);
    assert!(
        !body.contains("invalid.test/a"),
        "an unvalidatable chunk is never written: {body}"
    );

    let records = store.load().expect("record store");
    let record = &records["https://invalid.test/a"];
    assert_ne!(
        record.status,
        PageStatus::Committed,
        "a chunk that was never written must NOT be committed"
    );
    assert!(
        record.last_error.is_some(),
        "the failure must be recorded, not swallowed"
    );
}
