//! End-to-end smoke (AC-5.1) + ADR-B2/R2 tripwire.
//!
//! Runs the full Tier A pipeline in-process: corpus → runner → aggregator →
//! cost → report. Asserts the report is valid Markdown with every strategy
//! row populated and correctly tier-labeled, and — critically — that the
//! engine summary line (`crawl completed`) actually reached each produced
//! JSONL trace. If that tripwire ever fails, the declared ADR-B2 escape hatch
//! (`bench_run` subprocess binary) must be invoked — NEVER weaken this test.
//!
//! Run: cargo nextest run -p webfang_benchmark --test e2e_smoke_test

use webfang_benchmark::aggregate::CrawlSummary;
use webfang_benchmark::corpus;
use webfang_benchmark::cost::{self, CostConfig};
use webfang_benchmark::report;
use webfang_benchmark::runner;
use webfang_core::domain::JsStrategy;

/// Full pipeline smoke: every strategy produces metrics AND a real engine
/// summary captured from its JSONL trace (tripwire against off-thread span
/// emission hiding required shapes).
#[test]
fn full_pipeline_produces_report_and_engine_summaries() {
    // Sync context on purpose: `run_all` owns its current-thread runtimes.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    // Obtain a manifest instance (run_all serves fresh corpora per run).
    let handle = rt.block_on(corpus::serve()).expect("corpus server");
    let outcomes = runner::run_all(&handle.manifest).expect("pipeline succeeds");

    assert_eq!(outcomes.len(), 3, "one outcome per strategy");
    let mut seen = std::collections::HashSet::new();

    let config = CostConfig::default();
    let mut metrics_all = Vec::new();

    for outcome in &outcomes {
        assert!(seen.insert(outcome.strategy), "duplicate strategy run");

        // TRIPWIRE (ADR-B2/R2): the summary below was lifted from the run's
        // own JSONL trace; `aggregate::parse_file` + `summary_of` error out
        // when the `crawl completed` line is absent, so reaching this point
        // means the summary line EXISTS in the produced JSONL.
        let summary: &CrawlSummary = &outcome.summary;
        assert!(
            summary.total_pages > 0,
            "{:?}: engine summary reports zero attempted pages",
            outcome.strategy
        );
        assert_eq!(
            summary.total_pages,
            summary.succeeded + summary.errors,
            "{:?}: summary accounting broken",
            outcome.strategy
        );

        // Metrics sanity across the measured dimensions.
        let m = &outcome.metrics;
        assert!(m.success_rate > 0.0, "{:?}: zero success rate", m.strategy);
        assert!(
            m.ram_cost_bytes > 0,
            "{:?}: RAM proxy not captured",
            m.strategy
        );
        assert!(
            m.pages_per_sec > 0.0,
            "{:?}: throughput missing",
            m.strategy
        );

        // Cost dimension computes from real run data (FR-4).
        let estimate = cost::estimate(m, &config).expect("cost estimate");
        assert!(estimate.webfang_usd_per_1k >= 0.0);

        metrics_all.push(m.clone());
    }

    // All three strategies ran, in the deterministic order.
    let order: Vec<JsStrategy> = outcomes.iter().map(|o| o.strategy).collect();
    assert_eq!(
        order,
        vec![JsStrategy::Static, JsStrategy::Hybrid, JsStrategy::Full],
        "strategies must run in deterministic order"
    );

    // Final stage: Markdown report (AC-5.1 shape assertions live here too).
    let md = report::render(&metrics_all, &config).expect("render");

    assert!(
        md.starts_with("# WebFang Benchmark Report"),
        "not Markdown-titled"
    );
    for strategy in ["Static", "Hybrid", "Full"] {
        assert!(
            md.contains(strategy),
            "report missing strategy row {strategy}"
        );
    }
    let labels = md.matches("simulated challenge corpus").count();
    assert!(
        labels >= 3,
        "tier labels missing from report rows ({labels})"
    );
    // Verbatim assumptions survive the full pipeline (AC-4.1).
    assert!(md.contains(config.infra.source_url));
    assert!(md.contains(config.crawl4ai.source_url));
    // Real-run traces satisfy the same parser contract as the committed goldens
    // (cross-check per T9; regeneration of goldens only with PR-body rationale).
    //
    // DELIBERATELY part of the same #[test] as the pipeline smoke: nextest runs
    // every test in its OWN process, so two browser-capable tests would race
    // Chrome's ProcessSingleton lock on chromiumoxide's fixed profile dir
    // (`/tmp/chromiumoxide-runner`) across process boundaries — invisible to
    // any in-process mutex. Merging them makes this file launch exactly ONE
    // browser-capable pipeline, eliminating the race by construction.
    let static_run = &outcomes[0];
    // Golden contract fields (ADR-B6): all 8 buckets + duration + throughput.
    let s = &static_run.summary;
    let buckets_sum = s.errors_waf
        + s.errors_http
        + s.errors_timeout
        + s.errors_network
        + s.errors_rate_limit
        + s.errors_extraction
        + s.errors_internal
        + s.errors_panic;
    assert_eq!(
        buckets_sum, s.errors,
        "bucket passthrough must sum to errors"
    );
    assert!(s.pages_per_sec > 0.0 && s.duration_secs >= 0.0);
}
