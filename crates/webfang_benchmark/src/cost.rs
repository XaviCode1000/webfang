//! Harness-side cost model (FR-4).
//!
//! Maps measured resource proxies (throughput, static RAM estimate) into
//! $-per-1k-pages under the declared assumptions in [`config::CostConfig`].
//! This module contains ZERO numeric literals: every constant dereferences the
//! config, so a pricing change touches only `cost/config.rs` (AC-4.2).

pub mod config;

pub use config::CostConfig;

use crate::aggregate::StrategyMetrics;
use crate::error::{BenchmarkError, Result};

/// Cost figures for one strategy run. Competitor cells are methodology
/// placeholders in slice 1 (spec FR-5); real mappings land in slice 2.
#[derive(Debug, Clone, PartialEq)]
pub struct CostEstimate {
    /// WebFang $ per 1000 pages under the declared infra assumptions.
    pub webfang_usd_per_1k: f64,
}

/// Estimate $/1k pages for one run.
///
/// Formula (design §4, printed verbatim in the report header):
/// `$/1k = hourly × (1000 / pages_per_sec / 3600)   // compute share`
///      `+ hourly × (ram_bytes / (ram_gb × 2^30))   // RAM-share amortization`
///
/// # Errors
///
/// [`BenchmarkError::CostConfig`] when throughput is non-positive (no
/// meaningful $/1k figure exists; fail loudly instead of emitting infinity).
pub fn estimate(metrics: &StrategyMetrics, config: &CostConfig) -> Result<CostEstimate> {
    if metrics.pages_per_sec <= 0.0 {
        return Err(BenchmarkError::CostConfig(format!(
            "non-positive throughput ({}) cannot price a run",
            metrics.pages_per_sec
        )));
    }

    let hourly = config.infra.instance_hourly_usd;
    let compute_share = hourly * ((1000.0 / metrics.pages_per_sec) / (60.0 * 60.0));
    let ram_share = hourly
        * (metrics.ram_cost_bytes as f64
            / (config.infra.instance_ram_gb * 1024.0 * 1024.0 * 1024.0));

    Ok(CostEstimate {
        webfang_usd_per_1k: compute_share + ram_share,
    })
}

/// Firecrawl $/1k pages for one public credit tier.
///
/// Formula (design §4, slice 2): `$/1k = tier_usd x 1000 / (tier_credits x
/// pages_per_credit)` — the tier's monthly price spread over the pages its
/// credit allotment yields under the declared page-per-credit assumption.
///
/// # Errors
///
/// [`BenchmarkError::CostConfig`] when `tier_index` is out of range or a
/// configured divisor is non-positive (fail loudly instead of dividing by
/// zero).
pub fn firecrawl_usd_per_1k(tier_index: usize, config: &CostConfig) -> Result<f64> {
    let tier_usd = *config
        .firecrawl
        .tier_usd
        .get(tier_index)
        .ok_or_else(|| {
BenchmarkError::CostConfig(format!(
"firecrawl tier index {tier_index} out of range (0..={})",
config.firecrawl.tier_usd.len() - 1
))
        })?;
    let tier_credits = config.firecrawl.tier_credits[tier_index];
    if tier_credits <= 0.0 || config.firecrawl.credits_per_page_assumption <= 0.0 {
        return Err(BenchmarkError::CostConfig(
"firecrawl tier credits and pages-per-credit must be positive".to_string(),
        ));
    }
    let pages_yielded = tier_credits * config.firecrawl.credits_per_page_assumption;
    Ok(tier_usd * 1000.0 / pages_yielded)
}

/// Crawl4AI $/1k pages: the same infra formula as [`estimate`] applied to
/// the documented self-host sizing in [`config::Crawl4AiSizing`] instead of
/// the WebFang basis.
///
/// # Errors
///
/// [`BenchmarkError::CostConfig`] when throughput is non-positive (same
/// rationale as [`estimate`]).
pub fn crawl4ai_usd_per_1k(
    pages_per_sec: f64,
    ram_cost_bytes: usize,
    config: &CostConfig,
) -> Result<f64> {
    if pages_per_sec <= 0.0 {
        return Err(BenchmarkError::CostConfig(format!(
"non-positive throughput ({pages_per_sec}) cannot price a run"
        )));
    }
    let hourly = config.crawl4ai.instance_hourly_usd;
    let compute_share = hourly * ((1000.0 / pages_per_sec) / (60.0 * 60.0));
    let ram_share = hourly
        * (ram_cost_bytes as f64 / (config.crawl4ai.instance_ram_gb * 1024.0 * 1024.0 * 1024.0));
    Ok(compute_share + ram_share)
}
