//! Acceptance test for issue #704 (Paso 1): every `tokio::spawn` /
//! `JoinSet::spawn` hot-path site must attach the spawned future to the
//! current span via `tracing::Instrument::in_current_span()`.
//!
//! `FileTraceLayer` derives the top-level JSONL `trace_id` from the
//! scope-root span ID (`logical_trace_id`). A spawn-orphaned future that
//! emits events outside any span (or under a parentless span) therefore
//! mints a DIFFERENT root — a second `trace_id` in the same run.
//!
//! This test runs a `--batch` crawl of two wiremock pages with
//! `--trace-file` and asserts the JSONL contains exactly ONE distinct
//! top-level `trace_id`: the run-root identity shared by every event.
//! Before the `in_current_span()` fixes, the batch processor's
//! `JoinSet::spawn` workers and the collector/sink writer tasks produced
//! additional orphan roots.
//!
//! Run with: cargo nextest run --test trace_orphan_spawn_test

#[path = "common/cli_harness.rs"]
mod common;
use common::cmd;

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serve `count` simple pages at `/page-<n>` and return the mock server.
async fn serve_pages(count: usize) -> MockServer {
    let server = MockServer::start().await;
    for n in 0..count {
        Mock::given(method("GET"))
            .and(path(format!("/page-{n}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><article>\
                    <h1>Orphan Trace Page {n}</h1>\
                    <p>Deterministic body for trace identity verification.</p>\
                 </article></body></html>"
            )))
            .mount(&server)
            .await;
    }
    server
}

/// Run a stdin batch crawl of `urls` with `--trace-file` into `trace_path`,
/// returning the number of events in the trace JSONL.
fn run_batch_with_trace(
    urls: &[String],
    output_dir: &std::path::Path,
    trace_path: &std::path::Path,
) -> usize {
    let stdin = urls.join("\n") + "\n";
    let assert = cmd()
        .arg("--batch")
        .arg("--output")
        .arg(output_dir)
        .arg("--trace-file")
        .arg(trace_path)
        .arg("--quiet")
        .write_stdin(stdin)
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    let file = std::fs::File::open(trace_path).unwrap_or_else(|e| {
        panic!("trace file must exist at {trace_path:?}: {e}\nstderr: {stderr}")
    });
    BufReader::new(file).lines().count()
}

/// Extract every distinct top-level `trace_id` from the JSONL trace file.
fn distinct_trace_ids(trace_path: &std::path::Path) -> BTreeSet<String> {
    let file = std::fs::File::open(trace_path)
        .unwrap_or_else(|e| panic!("trace file must exist at {trace_path:?}: {e}"));
    BufReader::new(file)
        .lines()
        .map(|line| line.expect("read trace line"))
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(&line)
                .unwrap_or_else(|e| panic!("trace line must be valid JSON: {e}\nline: {line}"))
        })
        .filter_map(|value| {
            value
                .get("trace_id")
                .and_then(|t| t.as_str())
                .map(str::to_owned)
        })
        .collect()
}

/// A two-page batch crawl with `--trace-file` must produce exactly ONE
/// distinct top-level `trace_id` — every event belongs to the run root.
#[tokio::test]
async fn batch_crawl_emits_a_single_trace_id() {
    let server = serve_pages(2).await;

    let out = TempDir::new().expect("create temp output dir");
    let trace_path = out.path().join("trace.jsonl");
    let urls = vec![
        format!("{}/page-0", server.uri()),
        format!("{}/page-1", server.uri()),
    ];

    let line_count = run_batch_with_trace(&urls, out.path(), &trace_path);
    assert!(
        line_count > 0,
        "the batch run must produce at least one trace event"
    );

    let trace_ids = distinct_trace_ids(&trace_path);
    assert!(
        !trace_ids.is_empty(),
        "every trace event must carry a top-level trace_id"
    );
    assert_eq!(
        trace_ids.len(),
        1,
        "every event must share the run-root trace_id: got {} distinct values: {trace_ids:?} \
         across {line_count} events",
        trace_ids.len()
    );
}
