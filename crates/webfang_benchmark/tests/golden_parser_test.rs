//! AC-3.3 / AC-3.4 — golden-file pinning of the trace parser.
//!
//! The goldens under `tests/goldens/` capture the FileTraceLayer JSONL shapes
//! (normative contract: `scripts/analyze-trace.sh`), with timestamps zeroed.
//! Any shape drift breaks these tests. Regeneration requires PR-body rationale
//! (ADR-B6). Run: cargo nextest run -p webfang_benchmark --test golden_parser_test

use std::path::PathBuf;

use webfang_benchmark::aggregate::{self, TraceRecord};
use webfang_benchmark::BenchmarkError;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("goldens")
        .join(name)
}

#[test]
fn parses_full_golden_exactly() {
    let records = aggregate::parse_file(&golden("golden_full.jsonl")).expect("parses");

    let summary = records.iter().find_map(|r| match r {
        TraceRecord::Summary(s) => Some(s.clone()),
        _ => None,
    });
    let s = summary.expect("golden_full contains the crawl completed summary");
    assert_eq!(s.total_pages, 10);
    assert_eq!(s.succeeded, 7);
    assert_eq!(s.errors, 3);
    assert_eq!(s.errors_waf, 1);
    assert_eq!(s.errors_http, 1);
    assert_eq!(s.errors_timeout, 1);
    assert_eq!(s.errors_network, 0);
    assert_eq!(s.errors_rate_limit, 0);
    assert_eq!(s.errors_extraction, 0);
    assert_eq!(s.errors_internal, 0);
    assert_eq!(s.errors_panic, 0);
    assert!((s.duration_secs - 2.0).abs() < f64::EPSILON);
    assert!((s.pages_per_sec - 5.0).abs() < f64::EPSILON);

    // Selective parsing: span_close records lifted, benign lines ignored.
    let mut span_closes: Vec<f64> = records
        .iter()
        .filter_map(|r| match r {
            TraceRecord::SpanClose { duration_ms } => Some(*duration_ms),
            _ => None,
        })
        .collect();
    span_closes.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    assert_eq!(span_closes, vec![12.0, 34.0, 56.0]);

    let urls_failed = records
        .iter()
        .filter(|r| matches!(r, TraceRecord::UrlsFailed { .. }))
        .count();
    assert_eq!(urls_failed, 1, "the ERROR event carries a urls-failed url");
    assert!(
        records
            .iter()
            .any(|r| matches!(r, TraceRecord::WafEvent)),
        "the WAF challenge event is classified"
    );
    // Progress/link-discovery lines are ignored, not classified as shapes.
}

#[test]
fn parses_summary_only_golden() {
    let records = aggregate::parse_file(&golden("golden_summary_only.jsonl")).expect("parses");
    assert!(matches!(
        records.as_slice(),
        [TraceRecord::Summary(_)]
    ));
}

#[test]
fn parses_span_close_only_golden() {
    let records = aggregate::parse_file(&golden("golden_span_close_only.jsonl")).expect("parses");
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .all(|r| matches!(r, TraceRecord::SpanClose { .. })));
}

/// AC-3.4 — missing summary fails loudly with a typed error naming the file.
/// Parsing itself is lenient (partial shapes are accepted, design §5); the
/// typed failure comes from summary extraction, which the pipeline requires.
#[test]
fn missing_summary_is_typed_error() {
    let path = golden("bad_missing_summary.jsonl");
    let records = aggregate::parse_file(&path).expect("lenient parse succeeds");
    let err = aggregate::summary_of(&records, path.to_string_lossy().as_ref())
        .expect_err("must fail");
    match err {
        BenchmarkError::MissingSummary { path: p } => {
            assert!(p.ends_with("bad_missing_summary.jsonl"), "got: {p}");
        }
        other => panic!("expected MissingSummary, got: {other:?}"),
    }
}

/// AC-3.4 — invalid JSON mid-file fails with the exact line number.
#[test]
fn invalid_json_reports_line_number() {
    let err = aggregate::parse_file(&golden("bad_invalid_json_midfile.jsonl")).expect_err("must fail");
    match err {
        BenchmarkError::Jsonl { line, .. } => assert_eq!(line, 2, "error must point at line 2"),
        other => panic!("expected Jsonl, got: {other:?}"),
    }
}

/// AC-3.4 — an unknown error-bucket key is an unexpected record shape at its line.
#[test]
fn unknown_bucket_reports_shape_error_with_line() {
    let err = aggregate::parse_file(&golden("bad_unknown_bucket.jsonl")).expect_err("must fail");
    match err {
        BenchmarkError::Shape { line, detail } => {
            assert_eq!(line, 1);
            assert!(detail.contains("errors_banana"), "detail: {detail}");
        }
        other => panic!("expected Shape, got: {other:?}"),
    }
}
