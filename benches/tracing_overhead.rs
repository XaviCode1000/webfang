//! Tracing overhead benchmark (observability roadmap #356, Fase 2).
//!
//! Measures the cost of the `#[instrument]` tracing added to the hot paths,
//! comparing a normal run (no subscriber) against a `--trace-file` run
//! (`FileTraceLayer` active).
//!
//! Methodology:
//! - A representative instrumented hot-path unit mirrors the codebase pattern
//!   (`#[instrument(skip_all, fields(...))]` + field recording + one event).
//! - `disabled`: no active subscriber — what a plain `webfang` run does.
//!   `#[instrument]` still builds the span but it is immediately disabled.
//! - `file_trace`: a `FileTraceLayer` subscriber scoped around the whole
//!   measurement loop via `with_default` — what `--trace-file` does, including
//!   the JSONL serialization and file I/O.
//!
//! The ratio `file_trace / disabled - 1` is the cost of enabling tracing.
//! Acceptance (Fase 2): < 5% on representative page-sized work. Note a real
//! crawl page does strictly MORE work than this unit (HTTP fetch + parse +
//! extraction), so real-world overhead is at or below what is measured here.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use tracing::instrument;
use tracing_subscriber::layer::SubscriberExt;
use webfang_core::infrastructure::observability::FileTraceLayer;

/// A representative instrumented hot-path unit: scans a body (like content or
/// WAF scanning), records span fields, and emits one event — mirroring what a
/// `crawl_page` / `scrape` span does per page.
#[instrument(skip_all, fields(url = "https://example.com/article", stage = "bench"))]
fn instrumented_page_unit(body: &str) -> usize {
    let mut acc = 0usize;
    for b in body.as_bytes() {
        acc = acc.wrapping_add(usize::from(*b));
        acc ^= acc.rotate_left(7);
    }
    tracing::debug!(
        bytes = body.len(),
        checksum = acc % 100_000,
        "page processed"
    );
    acc
}

/// Builds a ~`target_kb` KiB body from a realistic article paragraph.
fn sample_body(target_kb: usize) -> String {
    let paragraph = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
                     Sed do eiusmod tempor incididunt ut labore et dolore magna \
                     aliqua. Ut enim ad minim veniam, quis nostrud exercitation \
                     ullamco laboris nisi ut aliquip ex ea commodo consequat. ";
    let target = target_kb * 1024;
    let repeats = (target / paragraph.len()).max(1);
    paragraph.repeat(repeats)
}

fn bench_tracing_overhead(c: &mut Criterion) {
    // Trace file in the temp dir (usually tmpfs → low I/O variance).
    let trace_path = std::env::temp_dir().join("webfang_tracing_overhead_bench.jsonl");
    let layer = FileTraceLayer::new(trace_path.clone()).expect("trace file must open");
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    // Two work sizes: a light operation and a full-page-sized unit.
    for (label, body) in [("16kib", sample_body(16)), ("128kib", sample_body(128))] {
        let mut group = c.benchmark_group(format!("tracing_overhead/{label}"));
        group.throughput(Throughput::Bytes(body.len() as u64));

        group.bench_function("disabled", |b| {
            b.iter(|| black_box(instrumented_page_unit(black_box(&body))))
        });

        // Subscriber scoped around the whole measurement loop (set once, as in
        // a real run), not per iteration.
        group.bench_function("file_trace", |b| {
            tracing::dispatcher::with_default(&dispatch, || {
                b.iter(|| black_box(instrumented_page_unit(black_box(&body))));
            })
        });

        group.finish();
    }

    let _ = std::fs::remove_file(&trace_path);
}

criterion_group!(benches, bench_tracing_overhead);
criterion_main!(benches);
