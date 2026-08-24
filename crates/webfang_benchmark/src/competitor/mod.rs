//! Live competitor adapter skeletons (slice 2, design §6).
//!
//! This slice ships compile-complete plumbing ONLY: request-building and
//! validation logic plus a fail-closed live-run gate. NO HTTP client is
//! constructed and NO outbound request is executed anywhere in this change —
//! when Tier B execution lands, it MUST use wreq exclusively (C-3; never
//! reqwest) inside these adapters.
//!
//! Gate contract (NFR-4): a live run requires BOTH a non-empty provider API
//! key in the environment AND an explicit CLI opt-in flag (`--i-understand-costs`,
//! enforced by the `bench_live` binary). Anything less yields
//! [`BenchmarkError::LiveDisabled`] before any request is prepared or sent.
//! Live runs are binaries/scripts, never tests (NFR-4 guard inventory stays
//! untouched).

use crate::error::{BenchmarkError, Result};

/// Which live competitor target a `bench_live` invocation addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompetitorTarget {
    Firecrawl,
    Crawl4Ai,
}

impl CompetitorTarget {
    /// Parse a CLI `--target` value into a target.
    ///
    /// # Errors
    ///
    /// [`BenchmarkError::Render`] for unknown names (typed usage error).
    pub fn parse_name(name: &str) -> Result<Self> {
        match name {
            "firecrawl" => Ok(Self::Firecrawl),
            "crawl4ai" => Ok(Self::Crawl4Ai),
            other => Err(BenchmarkError::Render(format!(
                "unknown --target `{other}` (expected `firecrawl` or `crawl4ai`)"
            ))),
        }
    }

    /// Human-readable provider identifier used in errors and logs.
    #[must_use]
    pub fn provider_name(self) -> &'static str {
        match self {
            Self::Firecrawl => "firecrawl",
            Self::Crawl4Ai => "crawl4ai",
        }
    }

    /// Environment variable holding this provider's API key.
    #[must_use]
    pub fn env_var(self) -> &'static str {
        match self {
            Self::Firecrawl => "FIRECRAWL_API_KEY",
            Self::Crawl4Ai => "CRAWL4AI_API_KEY",
        }
    }
}

/// Pure live-run gate: opens only for key-present AND explicit opt-in.
///
/// The presence input is passed in (rather than read from the process
/// environment here) so the decision logic is unit-testable without mutating
/// global state.
///
/// # Errors
///
/// [`BenchmarkError::LiveDisabled`] unless both conditions hold.
pub fn check_live_gate(
    target: CompetitorTarget,
    key_present: bool,
    opt_in_flag: bool,
) -> Result<()> {
    if key_present && opt_in_flag {
        return Ok(());
    }
    Err(BenchmarkError::LiveDisabled {
        provider: target.provider_name(),
        env_var: target.env_var(),
    })
}

/// Convenience wrapper reading key presence from the process environment.
///
/// # Errors
///
/// [`BenchmarkError::LiveDisabled`] unless the env var is non-empty AND
/// `opt_in_flag` is true.
pub fn evaluate_live_gate(target: CompetitorTarget, opt_in_flag: bool) -> Result<()> {
    check_live_gate(target, env_key_present(target.env_var()), opt_in_flag)
}

/// True iff `env_var` is set to a non-empty, non-whitespace value.
pub(crate) fn env_key_present(env_var: &str) -> bool {
    std::env::var_os(env_var)
        .is_some_and(|value| !value.to_string_lossy().trim().is_empty())
}
