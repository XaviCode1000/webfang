//! Regression test for issue #519: span attribution must survive `.await`
//! under the multi-threaded runtime.
//!
//! Production spans in the crawl path use `#[instrument]` / `.instrument(span)`
//! (the pattern fixed in #519). That re-enters the span on EVERY poll — on
//! whichever worker thread polls the future — so `FileTraceLayer`'s
//! thread-local `SPAN_STACK` is always correct while the body runs. The
//! previous pattern held a `span.enter()` guard across `.await`: the guard
//! registered the span only on the creating thread, so after a worker-thread
//! hop the polling thread had an empty stack and every event lost its
//! `span_fields` attribution.
//!
//! Each round runs in its own `tokio::spawn`'d task (the `#[tokio::test]` body
//! itself is driven by `block_on` on the main thread and never migrates). The
//! task fills its worker's local queue and parks on a timer; the timer wakeup
//! is then stolen by a different worker — a real thread hop. The test asserts
//! EVERY event emitted inside the instrumented span still carries
//! `span_fields.correlation_id` and `span_fields.trace_id`.

use std::io::{BufRead, BufReader};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn span_attribution_survives_await_under_multi_thread_runtime() {
    let trace_path = std::env::temp_dir().join(format!(
        "webfang_span_attribution_{}.jsonl",
        std::process::id()
    ));

    INSTALL_GLOBAL_SUBSCRIBER.call_once(|| {
        let layer = FileTraceLayer::new(trace_path.clone()).unwrap();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::set_global_default(subscriber)
            .expect("global subscriber must not be installed twice in this test binary");
    });

    // Fixed IDs keep the assertions exact-equality and deterministic.
    let trace_id = uuid::Uuid::parse_str("01949e0e-8b8e-7000-8000-000000000001").unwrap();
    let correlation = CorrelationId::new_with_ids(trace_id, 0x0000_0000_0000_0042);
    let expected_traceparent = correlation.to_string();

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
    // must still be attributed to the span after all the awaits.
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

    let _ = std::fs::remove_file(&trace_path);
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
