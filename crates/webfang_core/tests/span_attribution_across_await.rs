//! Regression tests for span attribution under the multi-threaded runtime.
//!
//! Issue #519: span attribution must survive `.await`. Production spans in the
//! crawl path use `#[instrument]` / `.instrument(span)` (the pattern fixed in
//! #519). That re-enters the span on EVERY poll — on whichever worker thread
//! polls the future.
//!
//! Issue #1238 (F-28 trace reconstruction): `FileTraceLayer::on_event` derives
//! `span` / `span_id` / `parent_id` / top-level `trace_id` from the subscriber
//! `Context` / span scope — the same source `on_close` uses. No thread-local
//! span stack is consulted, so events keep attribution when Tokio moves a task
//! across worker threads. Top-level `trace_id` is the root span `Id` (16-hex),
//! single per run and EPHEMERAL to the process/run (identity-within-run, not a
//! durable global identity).
//!
//! Each round runs in its own `tokio::spawn`'d task (the `#[tokio::test]` body
//! itself is driven by `block_on` on the main thread and never migrates). The
//! task fills its worker's local queue and parks on a timer; the timer wakeup
//! is then stolen by a different worker — a real thread hop.

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Once;
use std::time::Duration;

use serde_json::Value;
use tracing::Instrument;
use tracing_subscriber::layer::SubscriberExt;

use webfang_core::domain::value_objects::CorrelationId;
use webfang_core::infrastructure::observability::FileTraceLayer;

/// This test binary runs in its own process, so a process-wide global
/// subscriber is safe — and required: Tokio worker threads resolve the
/// *global* default dispatcher, not a thread-local `with_default`, so events
/// are only captured on every polling thread if the layer is global.
static INSTALL_GLOBAL_SUBSCRIBER: Once = Once::new();

/// Shared trace path for this binary (pid-scoped). Both tests in this file
/// share the single global `FileTraceLayer`; the file is intentionally NOT
/// deleted so concurrent tests never race a removal. Filtering is by content
/// (`span_fields`), never by truncation.
fn trace_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "webfang_span_attribution_{}.jsonl",
        std::process::id()
    ))
}

fn ensure_global_subscriber() {
    let path = trace_path();
    INSTALL_GLOBAL_SUBSCRIBER.call_once(|| {
        let layer = FileTraceLayer::new(path).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::set_global_default(subscriber)
            .expect("global subscriber must not be installed twice in this test binary");
    });
}

fn fixed_correlation() -> CorrelationId {
    let trace_id = uuid::Uuid::parse_str("01949e0e-8b8e-7000-8000-000000000001").unwrap();
    CorrelationId::new_with_ids(trace_id, 0x0000_0000_0000_0042)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn span_attribution_survives_await_under_multi_thread_runtime() {
    ensure_global_subscriber();
    let trace_path = trace_path();

    // Fixed IDs keep the assertions exact-equality and deterministic.
    let correlation = fixed_correlation();
    let expected_traceparent = correlation.to_string();
    let trace_id = correlation.trace_id();

    // Multiple rounds: each round mints a fresh span and forces at least one
    // thread hop, so a reintroduced enter-guard pattern loses attribution on
    // at least one post-hop event.
    for _ in 0..12 {
        let span = tracing::info_span!(
            "crawl_page",
            url = "https://example.com/page",
            correlation_id = %correlation,
            trace_id = %correlation.trace_id(),
        );

        let work = async move {
            tracing::info!("before first await");

            // Saturate every worker with longer-lived tasks so THIS task's
            // timer wakeup cannot be serviced by its owning worker and must be
            // stolen by a different one — a real thread hop.
            let mut flood = Vec::new();
            for _ in 0..128 {
                flood.push(tokio::spawn(async {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }));
            }

            tokio::time::sleep(Duration::from_millis(2)).await;
            tracing::info!("after timer hop");

            for handle in flood {
                handle.await.expect("flood task must complete");
            }
            tracing::info!("after flood drain");
        };
        tokio::spawn(work.instrument(span))
            .await
            .expect("spawned instrumented task must complete");
    }

    // Every event record (events have a `message`; span_close records do not)
    // must still be attributed to the span after all the awaits. Events from
    // the sibling #1238 test share this file but reuse the same fixed
    // correlation, so this assertion holds for the whole file.
    let records = read_jsonl_records(&trace_path);
    let events: Vec<&Value> = records
        .iter()
        .filter(|r| r["message"].is_string())
        .collect();
    assert!(
        !events.is_empty(),
        "the trace file must contain event records"
    );

    for event in events {
        let span_fields = event["span_fields"]
            .as_object()
            .unwrap_or_else(|| panic!("event lost span attribution after an await: {event}"));
        assert_eq!(
            span_fields["correlation_id"].as_str(),
            Some(expected_traceparent.as_str()),
            "correlation_id must be re-declared on the polling thread after each await: {event}"
        );
        assert_eq!(
            span_fields["trace_id"].as_str(),
            Some(trace_id.to_string().as_str()),
            "trace_id must be re-declared on the polling thread after each await: {event}"
        );
    }
}

/// Issue #1238: a simulated `multi_thread(4)` run whose FULL JSONL is
/// reconstructable from the single top-level `trace_id` (root span `Id`,
/// 16-hex, EPHEMERAL to the run).
///
/// Asserts: every record for this run carries one 16-hex top-level
/// `trace_id` equal to the root span id; every non-`run` record carries a
/// `parent_id` present in the run's `span_id` set; `span_close` count equals
/// distinct-span count; `select(.trace_id == ROOT)` returns all pages plus
/// errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trace_reconstruction_single_logical_trace_id_per_multithread_run_1238() {
    ensure_global_subscriber();
    let path = trace_path();
    let correlation = fixed_correlation();
    let run_marker = format!("run-1238-{}", std::process::id());

    let root_span = tracing::info_span!(
        "run",
        correlation_id = %correlation,
        trace_id = %correlation.trace_id(),
        run_marker = %run_marker,
    );
    let root_hex = root_span
        .id()
        .map(|id| format!("{:016x}", id.into_u64()))
        .expect("root span must have an Id");

    run_pages_under_root(root_span, &run_marker).await;

    let records = read_jsonl_records(&path);
    let run_records = filter_run_records(&records, &run_marker);
    assert_run_reconstructable(&records, &run_records, &root_hex);
}

async fn run_pages_under_root(root_span: tracing::Span, run_marker: &str) {
    let marker = run_marker.to_string();
    let run_future = async move {
        let mut handles = Vec::new();
        for page in 0..4 {
            let marker_clone = marker.clone();
            let page_span = tracing::info_span!(
                "crawl_page",
                url = format!("https://example.com/page{page}"),
                page_index = page,
                run_marker = %marker_clone,
            );
            let work = page_work(page);
            handles.push(tokio::spawn(work.instrument(page_span)));
        }
        for handle in handles {
            handle.await.expect("page task must complete");
        }
        tracing::info!("run complete");
    };
    tokio::spawn(run_future.instrument(root_span))
        .await
        .expect("root task must complete");
}

async fn page_work(page: u32) {
    tracing::info!(page = page, "fetching page");
    let flood = spawn_flood(64);
    tokio::time::sleep(Duration::from_millis(2)).await;
    tracing::info!(page = page, "after hop");
    maybe_emit_page_error(page);
    drain_flood(flood).await;
    tracing::info!(page = page, "page done");
}

fn spawn_flood(count: usize) -> Vec<tokio::task::JoinHandle<()>> {
    let mut flood = Vec::with_capacity(count);
    for _ in 0..count {
        flood.push(tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }));
    }
    flood
}

async fn drain_flood(flood: Vec<tokio::task::JoinHandle<()>>) {
    for handle in flood {
        handle.await.expect("flood task must complete");
    }
}

fn maybe_emit_page_error(page: u32) {
    if page.is_multiple_of(2) {
        tracing::error!(page = page, error = "boom", "page failed");
    }
}

fn filter_run_records<'a>(records: &'a [Value], marker: &str) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|r| r["span_fields"]["run_marker"].as_str() == Some(marker))
        .collect()
}

fn assert_run_reconstructable(all_records: &[Value], run_records: &[&Value], root_hex: &str) {
    assert!(!run_records.is_empty(), "run must emit records");
    assert_valid_trace_ids(run_records, root_hex);
    assert_parent_links(run_records);
    assert_close_counts(run_records);
    assert_select_by_trace_id(all_records, run_records, root_hex);
}

fn assert_valid_trace_ids(run_records: &[&Value], root_hex: &str) {
    for record in run_records {
        let trace_id = record["trace_id"]
            .as_str()
            .unwrap_or_else(|| panic!("record must carry top-level trace_id: {record}"));
        assert_eq!(
            trace_id.len(),
            16,
            "trace_id must be 16 hex chars: {record}"
        );
        assert!(
            trace_id.chars().all(|c| c.is_ascii_hexdigit()),
            "trace_id must be hex: {record}"
        );
        assert_eq!(
            trace_id, root_hex,
            "every run record must share the root span id: {record}"
        );
    }
}

fn assert_parent_links(run_records: &[&Value]) {
    let span_ids: BTreeSet<&str> = run_records
        .iter()
        .filter_map(|r| r["span_id"].as_str())
        .collect();
    for record in run_records {
        if record["span"].as_str() == Some("run") {
            assert!(
                record["parent_id"].is_null(),
                "root run record must have no parent_id: {record}"
            );
        } else {
            let parent_id = record["parent_id"]
                .as_str()
                .unwrap_or_else(|| panic!("non-run record must carry parent_id: {record}"));
            assert!(
                span_ids.contains(parent_id),
                "parent_id must resolve to a run span_id: {record}"
            );
        }
    }
}

fn assert_close_counts(run_records: &[&Value]) {
    let closes: Vec<&&Value> = run_records
        .iter()
        .filter(|r| r["record"].as_str() == Some("span_close"))
        .collect();
    let distinct: BTreeSet<&str> = run_records
        .iter()
        .filter_map(|r| r["span_id"].as_str())
        .collect();
    assert_eq!(
        closes.len(),
        distinct.len(),
        "span_close count must equal distinct-span count"
    );
    assert_eq!(
        closes.len(),
        5,
        "expected 1 run + 4 page spans to close, got {}",
        closes.len()
    );
}

fn assert_select_by_trace_id(all_records: &[Value], run_records: &[&Value], root_hex: &str) {
    let selected: Vec<&Value> = all_records
        .iter()
        .filter(|r| r["trace_id"].as_str() == Some(root_hex))
        .collect();
    assert_eq!(
        selected.len(),
        run_records.len(),
        "select(.trace_id == ROOT) must return exactly this run"
    );
    assert!(
        selected
            .iter()
            .any(|r| r["span"].as_str() == Some("crawl_page")),
        "reconstructed run must contain page spans"
    );
    assert!(
        selected.iter().any(|r| r["level"] == "ERROR"),
        "reconstructed run must contain error events"
    );
}

/// Read the JSONL trace back as parsed records.
fn read_jsonl_records(path: &std::path::Path) -> Vec<Value> {
    let file = std::fs::File::open(path).expect("trace file must exist");
    let reader = BufReader::new(file);
    reader
        .lines()
        .map(|line| {
            let line = line.expect("line must be readable");
            serde_json::from_str(&line).expect("each trace line must be a JSON object")
        })
        .collect()
}
