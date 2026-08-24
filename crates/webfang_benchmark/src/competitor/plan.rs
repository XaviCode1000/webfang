//! Live-run planning guards (Tier B Step 0): projected-credit budgeting,
//! free-plan concurrency clamping, and inter-request pacing.
//!
//! Pure library-level logic so the guards are unit-testable without running
//! the `bench_live` binary — and so the typed budget refusal happens during
//! PLANNING, before any request is prepared or sent.

use super::CompetitorTarget;
use crate::error::{BenchmarkError, Result};

/// Default live-pass credit budget guard (`--max-credits`) sized against the
/// free plan's 1000 monthly credits: calibration (~30) + full pass (~150) +
/// retry margin. Mirrors `tierb_corpus::DEFAULT_MAX_CREDITS` (same constant,
/// same approved plan).
pub const DEFAULT_MAX_CREDITS: u32 = 250;

/// Firecrawl free-plan hard concurrency ceiling
/// (`FreeTierPlan::max_concurrent`); any requested value above this is
/// clamped down for the firecrawl target.
pub const FIRECRAWL_MAX_CONCURRENCY: u32 = 2;

/// Inter-request delay applied between consecutive requests of a live pass,
/// present in every plan output so pacing is explicit, never implicit.
pub const DEFAULT_INTER_REQUEST_DELAY_MS: u64 = 1000;

/// A validated live-run plan: what would run, at what cost projection, with
/// which effective concurrency and pacing. Producing a plan performs NO I/O.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRunPlan {
    /// Selected live competitor target.
    pub target: CompetitorTarget,
    /// Total pages the planned pass would attempt.
    pub total_pages: u32,
    /// Projected credit spend (`total_pages x credit_cost_per_page`).
    pub projected_credits: f64,
    /// Budget guard the projection was validated against.
    pub max_credits: u32,
    /// Concurrency as requested on the command line (pre-clamp).
    pub requested_concurrency: u32,
    /// Effective concurrency after any provider clamp.
    pub concurrency: u32,
    /// Delay between consecutive requests (milliseconds).
    pub delay_ms: u64,
}

/// Plan a live run under the Step 0 guards.
///
/// - **Budget**: projected credits (`total_pages * credit_cost_per_page`)
///   above `max_credits` yield [`BenchmarkError::BudgetExceeded`] before any
///   request exists. Use `credit_cost_per_page = 0.0` for self-hosted targets
///   without a credit meter.
/// - **Concurrency**: requested concurrency is floored at 1; for the
///   Firecrawl target it is additionally hard-clamped to
///   [`FIRECRAWL_MAX_CONCURRENCY`] (free-plan ceiling).
/// - **Pacing**: the plan always carries [`DEFAULT_INTER_REQUEST_DELAY_MS`].
///
/// # Errors
///
/// [`BenchmarkError::BudgetExceeded`] when the projection exceeds the guard.
pub fn plan_live_run(
    target: CompetitorTarget,
    total_pages: u32,
    requested_concurrency: u32,
    max_credits: u32,
    credit_cost_per_page: f64,
) -> Result<LiveRunPlan> {
    let projected_credits = f64::from(total_pages) * credit_cost_per_page;
    if projected_credits > f64::from(max_credits) {
        return Err(BenchmarkError::BudgetExceeded {
            provider: target.provider_name(),
            projected_credits,
            budget: max_credits,
        });
    }

    let requested_concurrency = requested_concurrency.max(1);
    let concurrency = match target {
        CompetitorTarget::Firecrawl => requested_concurrency.min(FIRECRAWL_MAX_CONCURRENCY),
        // Self-hosted Crawl4AI has no free-plan concurrency ceiling here.
        CompetitorTarget::Crawl4Ai => requested_concurrency,
    };

    Ok(LiveRunPlan {
        target,
        total_pages,
        projected_credits,
        max_credits,
        requested_concurrency,
        concurrency,
        delay_ms: DEFAULT_INTER_REQUEST_DELAY_MS,
    })
}
