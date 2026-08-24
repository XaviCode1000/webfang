//! Slice-2 competitor pricing math (FR-4, design §4).
//!
//! Firecrawl maps public credit tiers into $/1k pages:
//! `tier_usd[i] x 1000 / (tier_credits[i] x pages_per_credit)`.
//! Crawl4AI reuses the WebFang infra formula over documented self-host sizing.
//!
//! Hand-computed expectations over fixed configs; all constants dereference
//! [`CostConfig`] so pricing edits stay single-source (AC-4.2).
//!
//! Run: cargo nextest run -p webfang_benchmark --test competitor_pricing_test

use webfang_benchmark::cost;
use webfang_benchmark::cost::config::{CostConfig, FreeTierPlan};

/// Free-tier plan limits are pinned single-source values (strategy/
/// tier-b-live-plan): 1000 credits/month, concurrency 2, no rollover,
/// 1 credit per scrape/crawl/map page, 2 per search-10, 2 per interact
/// minute, failed requests not charged.
#[test]
fn free_tier_plan_pins_documented_limits() {
    let plan = FreeTierPlan::default();
    assert_eq!(plan.credits_monthly, 1000.0);
    assert_eq!(plan.max_concurrent, 2);
    assert!(!plan.rollover);
    assert_eq!(plan.scrape_crawl_map_cost_per_page, 1.0);
    assert_eq!(plan.search_cost_per_10_results, 2.0);
    assert_eq!(plan.interact_cost_per_minute, 2.0);
    assert!(!plan.failed_requests_charged);
    assert_eq!(plan.source_url, "https://www.firecrawl.dev/pricing");
    assert_eq!(plan.retrieved, "2026-08-24");
}

/// Shadow price translates free-plan usage into paid economics via the
/// Hobby tier: $16 / 5000 credits = $0.0032 per credit.
#[test]
fn free_tier_plan_shadow_price_pins_hobby_rate() {
    let plan = FreeTierPlan::default();
    let price = plan.shadow_price_usd_per_credit();
    assert!((price - 0.0032).abs() < 1e-12, "expected 0.0032, got {price}");
}

/// Fixed config for arithmetic proofs (NOT the shipped default).
fn fixed_config() -> CostConfig {
    let mut config = CostConfig::default();
    config.firecrawl.tier_usd = [16.0, 83.0, 333.0];
    config.firecrawl.tier_credits = [5_000.0, 100_000.0, 500_000.0];
    config.firecrawl.credits_per_page_assumption = 1.0;
    config.crawl4ai.instance_hourly_usd = 0.02;
    config.crawl4ai.instance_ram_gb = 4.0;
    config
}

/// Hobby tier: 16 USD x 1000 / (5000 credits x 1 page/credit) = 3.20 per 1k.
#[test]
fn firecrawl_hobby_tier_matches_hand_computation() {
    let usd = cost::firecrawl_usd_per_1k(0, &fixed_config()).expect("hobby tier prices");
    assert!((usd - 3.20).abs() < 1e-12, "expected 3.2, got {usd}");
}

/// Standard tier: 83 USD x 1000 / (100000 x 1) = 0.83 per 1k.
#[test]
fn firecrawl_standard_tier_matches_hand_computation() {
    let usd = cost::firecrawl_usd_per_1k(1, &fixed_config()).expect("standard tier prices");
    assert!((usd - 0.83).abs() < 1e-12, "expected 0.83, got {usd}");
}

/// Growth tier: 333 USD x 1000 / (500000 x 1) = 0.666 per 1k.
#[test]
fn firecrawl_growth_tier_matches_hand_computation() {
    let usd = cost::firecrawl_usd_per_1k(2, &fixed_config()).expect("growth tier prices");
    assert!((usd - 0.666).abs() < 1e-9, "expected 0.666, got {usd}");
}

/// Pages-per-credit yield scales the denominator linearly:
/// hobby at 2 pages/credit halves the figure to 1.60.
#[test]
fn firecrawl_yield_assumption_scales_price() {
    let mut config = fixed_config();
    config.firecrawl.credits_per_page_assumption = 2.0;
    let usd = cost::firecrawl_usd_per_1k(0, &config).expect("hobby tier prices");
    assert!((usd - 1.60).abs() < 1e-12, "expected 1.6, got {usd}");
}

/// Out-of-range tier index is a typed error, never a panic.
#[test]
fn firecrawl_unknown_tier_is_typed_error() {
    let err = cost::firecrawl_usd_per_1k(3, &fixed_config()).expect_err("must fail");
    assert!(matches!(
        err,
        webfang_benchmark::BenchmarkError::CostConfig(_)
    ));
}

/// Crawl4AI applies the same infra formula over its own sizing:
/// 0.02 x (1000/10/3600) + 0.02 x (2^27 / (4 x 2^30)) = 5.555...e-4 + 6.25e-4.
#[test]
fn crawl4ai_infra_formula_matches_hand_computation() {
    let pages_per_sec = 10.0;
    let ram_bytes: usize = 1 << 27;
    let usd = cost::crawl4ai_usd_per_1k(pages_per_sec, ram_bytes, &fixed_config())
        .expect("crawl4ai prices");
    let expected = 0.02 * (1000.0 / pages_per_sec / 3600.0)
        + 0.02 * ((ram_bytes as f64) / (4.0 * 1024.0 * 1024.0 * 1024.0));
    assert!(
        (usd - expected).abs() < 1e-12,
        "expected {expected}, got {usd}"
    );
}

/// Non-positive throughput cannot price a self-hosted run either.
#[test]
fn crawl4ai_non_positive_throughput_is_typed_error() {
    let err = cost::crawl4ai_usd_per_1k(0.0, 0, &fixed_config()).expect_err("must fail");
    assert!(matches!(
        err,
        webfang_benchmark::BenchmarkError::CostConfig(_)
    ));
}
