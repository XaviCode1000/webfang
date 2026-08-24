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
