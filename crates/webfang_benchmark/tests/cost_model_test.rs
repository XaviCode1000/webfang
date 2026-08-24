//! FR-4 / AC-4.2 groundwork — harness-side cost model.
//!
//! Hand-computed expectations over a fixed config prove the formula
//! `$/1k = hourly × (1000/pages_per_sec/3600) + hourly × (ram_bytes/(ram_gb×2^30))`
//! (design §4) with no hardcoded drift: all constants dereference [`CostConfig`].
//!
//! Run: cargo nextest run -p webfang_benchmark --test cost_model_test

use webfang_benchmark::aggregate::{ErrorBuckets, StrategyMetrics};
use webfang_benchmark::cost;
use webfang_benchmark::cost::config::CostConfig;
use webfang_core::domain::JsStrategy;

/// Fixed config for arithmetic proofs (NOT the shipped default).
fn fixed_config() -> CostConfig {
    let mut config = CostConfig::default();
    config.infra.instance_hourly_usd = 0.10;
    config.infra.instance_ram_gb = 1.0;
    config
}

fn metrics(pages_per_sec: f64, ram_cost_bytes: usize) -> StrategyMetrics {
    StrategyMetrics {
        strategy: JsStrategy::Static,
        success_rate: 1.0,
        p50_ms: 0.0,
        p95_ms: 0.0,
        pages_per_sec,
        wall_clock_secs: 0.0,
        error_buckets: ErrorBuckets {
            waf: 0,
            http: 0,
            timeout: 0,
            network: 0,
            rate_limit: 0,
            extraction: 0,
            internal: 0,
            panic: 0,
        },
        ram_cost_bytes,
    }
}

/// Compute-share term only: 0.10 × (1000 / 100 / 3600) ≈ 2.778e-4.
#[test]
fn compute_share_matches_hand_computation() {
    // RAM term forced to zero (0 bytes): result isolates the compute share.
    let est = cost::estimate(&metrics(100.0, 0), &fixed_config()).expect("estimates");
    let expected = 0.10 * (1000.0 / 100.0 / 3600.0);
    assert!(
        (est.webfang_usd_per_1k - expected).abs() < 1e-12,
        "expected {expected}, got {}",
        est.webfang_usd_per_1k
    );
}

/// RAM-share amortization term only: 0.10 × (2^30 / (1 × 2^30)) = 0.10,
/// plus compute share ⇒ total 0.10 + 1/36000.
#[test]
fn full_formula_matches_hand_computation() {
    let est = cost::estimate(&metrics(100.0, 1 << 30), &fixed_config()).expect("estimates");
    let expected = 0.10 + 0.10 * (1000.0 / 100.0 / 3600.0);
    assert!(
        (est.webfang_usd_per_1k - expected).abs() < 1e-12,
        "expected {expected}, got {}",
        est.webfang_usd_per_1k
    );
}

/// Non-positive throughput cannot yield a meaningful $/1k figure: fail loudly.
#[test]
fn non_positive_throughput_is_typed_error() {
    let err = cost::estimate(&metrics(0.0, 0), &fixed_config()).expect_err("must fail");
    assert!(matches!(
        err,
        webfang_benchmark::BenchmarkError::CostConfig(_)
    ));
}
