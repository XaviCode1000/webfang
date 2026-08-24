//! FR-5 / AC-4.1 / AC-5.2 groundwork — Markdown report generation with
//! enforced tier labeling, verbatim assumption headers, and the ADR-B4
//! two-block reproducibility contract (`<!-- volatile -->` sentinels).
//!
//! Run: cargo nextest run -p webfang_benchmark --test report_test

use webfang_benchmark::aggregate::{ErrorBuckets, StrategyMetrics};
use webfang_benchmark::cost::config::CostConfig;
use webfang_benchmark::report;
use webfang_core::domain::JsStrategy;

fn sample(strategy: JsStrategy, ram_bytes: usize) -> StrategyMetrics {
    StrategyMetrics {
        strategy,
        success_rate: 0.7,
        p50_ms: 42.5,
        p95_ms: 96.25,
        pages_per_sec: 8.4,
        wall_clock_secs: 1.19,
        error_buckets: ErrorBuckets {
            waf: 1,
            http: 0,
            timeout: 0,
            network: 0,
            rate_limit: 0,
            extraction: 0,
            internal: 0,
            panic: 0,
        },
        ram_cost_bytes: ram_bytes,
    }
}

fn rendered() -> String {
    let metrics = vec![
        sample(JsStrategy::Static, 10 << 20),
        sample(JsStrategy::Hybrid, 40 << 20),
        sample(JsStrategy::Full, 200 << 20),
    ];
    report::render(&metrics, &CostConfig::default()).expect("renders")
}

/// AC-4.1 — all infra assumptions and competitor pricing sources appear
/// VERBATIM in the header.
#[test]
fn header_contains_assumption_sources_verbatim() {
    let config = CostConfig::default();
    let out = rendered();
    for expected in [
        config.infra.source_url,
        config.infra.retrieved,
        config.firecrawl.source_url,
        config.firecrawl.retrieved,
        config.crawl4ai.source_url,
        config.crawl4ai.retrieved,
    ] {
        assert!(
            out.contains(expected),
            "header must contain `{expected}` verbatim"
        );
    }
}

/// AC-5.2 — every Tier A row carries its honest label; mixing is forbidden.
#[test]
fn tier_a_rows_are_labeled_simulated_challenge_corpus() {
    let out = rendered();
    let labels = out.matches("simulated challenge corpus").count();
    assert!(
        labels >= 3,
        "each of the 3 strategy rows needs a tier label, got {labels}"
    );
}

/// AC-4.1/FR-5 — methodology notes state the D2 limitation and the percentile
/// convention (ADR-B3).
#[test]
fn methodology_notes_present() {
    let out = rendered();
    assert!(
        out.contains("no per-layer latency split"),
        "D2 limitation note missing"
    );
    assert!(
        out.contains("nearest-rank"),
        "percentile convention note missing"
    );
}

/// ADR-B4 — volatile quantities are wrapped in exactly one pair of
/// `<!-- volatile -->` sentinels, and live inside it.
#[test]
fn volatile_block_delimits_wall_clock_quantities() {
    let out = rendered();
    assert_eq!(
        out.matches("<!-- volatile -->").count(),
        2,
        "one open + one close sentinel"
    );

    let start = out.find("<!-- volatile -->").expect("open sentinel");
    let end = out.rfind("<!-- volatile -->").expect("close sentinel");
    let block_b = &out[start..end];
    for needle in ["p50", "p95", "pages/sec", "wall-clock"] {
        assert!(
            block_b.contains(needle),
            "`{needle}` must be inside the volatile block"
        );
    }
}

/// AC-5.2/NFR-1 — no environment leakage: temp paths, ports, hostnames, or
/// wall-clock timestamps may never reach compared output.
#[test]
fn report_contains_no_environment_leakage() {
    let out = rendered();
    for forbidden in ["/tmp", "127.0.0.1", "localhost", "tempfile", "20:"] {
        assert!(!out.contains(forbidden), "report leaked `{forbidden}`");
    }
}
