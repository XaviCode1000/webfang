//! AC-3.1 / AC-3.2 — metric computation over parsed records.
//! Percentile convention: nearest-rank, 1-based, NO interpolation (ADR-B3).
//! Run: cargo nextest run -p webfang_benchmark --test metrics_test

use webfang_benchmark::aggregate::{compute, CrawlSummary, TraceRecord};

fn summary(total_pages: u64, succeeded: u64) -> CrawlSummary {
    CrawlSummary {
        total_pages,
        succeeded,
        errors: total_pages - succeeded,
        errors_waf: 0,
        errors_http: 0,
        errors_timeout: 0,
        errors_network: 0,
        errors_rate_limit: 0,
        errors_extraction: 0,
        errors_internal: 0,
        errors_panic: 0,
        duration_secs: 2.0,
        pages_per_sec: f64::from(u32::try_from(total_pages).unwrap_or(0)) / 2.0,
        trace_id: Some("test-trace".to_string()),
    }
}

fn records_with(summary: CrawlSummary, durations_ms: &[f64]) -> Vec<TraceRecord> {
    let mut records = vec![TraceRecord::Summary(summary)];
    for d in durations_ms {
        records.push(TraceRecord::SpanClose { duration_ms: *d });
    }
    records
}

/// AC-3.1 — success rate is succeeded / total attempted.
#[test]
fn success_rate_is_succeeded_over_total() {
    let records = records_with(summary(10, 7), &[10.0]);
    let metrics =
        compute(&records, webfang_core::domain::JsStrategy::Static, 0).expect("computes");
    assert!((metrics.success_rate - 0.7).abs() < f64::EPSILON, "7/10 must be 70%");
}

/// AC-3.2 — nearest-rank percentiles: n=100 samples 1..=100ms → p50=50, p95=95.
#[test]
fn percentiles_are_nearest_rank_no_interpolation() {
    let durations: Vec<f64> = (1..=100).map(f64::from).collect();
    let records = records_with(summary(100, 100), &durations);
    let metrics =
        compute(&records, webfang_core::domain::JsStrategy::Static, 0).expect("computes");
    assert_eq!(metrics.p50_ms, 50.0);
    assert_eq!(metrics.p95_ms, 95.0);
}

/// Triangulation: n=10 samples {10,20,...,100} → p50 idx=5 → 50;
/// p95 idx=ceil(9.5)=10 → 100 (interpolation would give 95.5).
#[test]
fn percentiles_nearest_rank_small_sample() {
    let durations: Vec<f64> = (1..=10).map(|i| f64::from(i) * 10.0).collect();
    let records = records_with(summary(10, 10), &durations);
    let metrics =
        compute(&records, webfang_core::domain::JsStrategy::Static, 0).expect("computes");
    assert_eq!(metrics.p50_ms, 50.0);
    assert_eq!(metrics.p95_ms, 100.0);
}

/// Missing summary fails loudly (AC-3.4 family).
#[test]
fn missing_summary_is_error() {
    let records = vec![TraceRecord::SpanClose { duration_ms: 5.0 }];
    let err = compute(&records, webfang_core::domain::JsStrategy::Static, 0)
        .expect_err("no summary");
    assert!(matches!(err, webfang_benchmark::BenchmarkError::MissingSummary { .. }));
}

/// Zero attempted pages cannot produce a meaningful success rate.
#[test]
fn zero_attempted_pages_is_empty_crawl() {
    let records = records_with(summary(0, 0), &[]);
    let err = compute(&records, webfang_core::domain::JsStrategy::Static, 0)
        .expect_err("empty crawl");
    assert!(matches!(err, webfang_benchmark::BenchmarkError::EmptyCrawl));
}
