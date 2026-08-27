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

pub mod crawl4ai;
pub mod firecrawl;
pub mod plan;
pub mod tierb_corpus;

pub use crawl4ai::Crawl4AiConfig;
pub use firecrawl::FirecrawlConfig;
pub use plan::{
    plan_live_run, LiveRunPlan, DEFAULT_INTER_REQUEST_DELAY_MS, DEFAULT_MAX_CREDITS,
    FIRECRAWL_MAX_CONCURRENCY,
};
pub use tierb_corpus::{egress_type_from_env, CREDITS_PER_PAGE, EGRESS_TYPE_ENV_VAR};

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

/// Convenience wrapper gating on an already-read key value, so callers read
/// the provider environment variable exactly once and thread it through.
///
/// # Errors
///
/// [`BenchmarkError::LiveDisabled`] unless the key is non-blank AND
/// `opt_in_flag` is true.
pub fn evaluate_live_gate(
    target: CompetitorTarget,
    api_key: Option<&str>,
    opt_in_flag: bool,
) -> Result<()> {
    let key_present = api_key.map(str::trim).is_some_and(|key| !key.is_empty());
    check_live_gate(target, key_present, opt_in_flag)
}

/// Parsed `bench_live` invocation (validated, gate-ready).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchLiveArgs {
    /// Selected live competitor.
    pub target: CompetitorTarget,
    /// Explicit cost opt-in (`--i-understand-costs`).
    pub opt_in: bool,
    /// Credit budget guard (`--max-credits`); default
    /// [`plan::DEFAULT_MAX_CREDITS`].
    pub max_credits: u32,
    /// Requested concurrency (`--concurrency`); default 1, clamped later by
    /// the planner for provider limits.
    pub concurrency: u32,
}

/// Usage line for the `bench_live` binary.
pub const BENCH_LIVE_USAGE: &str =
    "usage: bench_live --target <firecrawl|crawl4ai> [--i-understand-costs] \
     [--max-credits <N>] [--concurrency <N>]";

/// Minimal arg parsing for `bench_live`, kept library-level so it is unit
/// testable (mirrors the hand-rolled `bench_tier_a` style; no clap dep).
///
/// Accepts exactly one `--target <firecrawl|crawl4ai>` and at most one
/// `--i-understand-costs`; anything else is a typed usage error.
///
/// # Errors
///
/// [`BenchmarkError::Render`] describing the usage violation.
pub fn parse_bench_live_args(cli_args: impl Iterator<Item = String>) -> Result<BenchLiveArgs> {
    let mut iter = cli_args.peekable();
    let mut target = None;
    let mut opt_in = false;
    let mut max_credits = plan::DEFAULT_MAX_CREDITS;
    let mut concurrency = 1;
    let mut seen_max_credits = false;
    let mut seen_concurrency = false;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--target" => {
                if target.is_some() {
                    return Err(BenchmarkError::Render(format!(
                        "{BENCH_LIVE_USAGE}: --target given more than once"
                    )));
                }
                let value = iter.next().ok_or_else(|| {
                    BenchmarkError::Render(format!("{BENCH_LIVE_USAGE}: --target needs a value"))
                })?;
                target = Some(CompetitorTarget::parse_name(&value)?);
            },
            "--i-understand-costs" => {
                if opt_in {
                    return Err(BenchmarkError::Render(format!(
                        "{BENCH_LIVE_USAGE}: --i-understand-costs given more than once"
                    )));
                }
                opt_in = true;
            },
            "--max-credits" | "--concurrency" => {
                let flag = arg.as_str();
                let seen = if arg == "--max-credits" {
                    &mut seen_max_credits
                } else {
                    &mut seen_concurrency
                };
                if *seen {
                    return Err(BenchmarkError::Render(format!(
                        "{BENCH_LIVE_USAGE}: {arg} given more than once"
                    )));
                }
                *seen = true;
                let value = iter.next().ok_or_else(|| {
                    BenchmarkError::Render(format!("{BENCH_LIVE_USAGE}: {flag} needs a value"))
                })?;
                let parsed: u32 = value.parse().map_err(|_| {
                    BenchmarkError::Render(format!(
                        "{BENCH_LIVE_USAGE}: {flag} expects a positive integer, got `{value}`"
                    ))
                })?;
                if parsed == 0 {
                    return Err(BenchmarkError::Render(format!(
                        "{BENCH_LIVE_USAGE}: {flag} must be >= 1"
                    )));
                }
                if arg == "--max-credits" {
                    max_credits = parsed;
                } else {
                    concurrency = parsed;
                }
            },
            other => {
                return Err(BenchmarkError::Render(format!(
                    "{BENCH_LIVE_USAGE}: unexpected argument `{other}`"
                )))
            },
        }
    }
    let target = target.ok_or_else(|| {
        BenchmarkError::Render(format!(
            "{BENCH_LIVE_USAGE}: --target <firecrawl|crawl4ai> is required"
        ))
    })?;
    Ok(BenchLiveArgs {
        target,
        opt_in,
        max_credits,
        concurrency,
    })
}

/// A fully-built HTTP request description.
///
/// Building this value performs NO I/O: it is pure data the future Tier B
/// execution layer will hand to a wreq client (C-3; never reqwest).
///
/// The manual [`Debug`] impl REDACTS [`Self::bearer_token`] as
/// `[REDACTED]`: prepared requests flow through logs and error paths, and
/// a provider API key must never appear in any rendered output.
#[derive(Clone)]
pub struct PreparedRequest {
    /// HTTP method of the deferred call.
    pub method: &'static str,
    /// Absolute endpoint URL, validated at construction time.
    pub url: ::url::Url,
    /// Provider API key (sent as `Authorization: Bearer <token>` on execute).
    pub bearer_token: String,
    /// JSON request body, already validated/normalized.
    pub body_json: serde_json::Value,
}

impl std::fmt::Debug for PreparedRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRequest")
            .field("method", &self.method)
            .field("url", &self.url)
            .field("bearer_token", &"[REDACTED]")
            .field("body_json", &self.body_json)
            .finish()
    }
}

/// Shared crawl-start parameters accepted by every adapter.
#[derive(Debug, Clone)]
pub struct StartCrawlParams {
    /// Absolute URL of the site to crawl.
    pub target_url: String,
    /// Maximum pages the remote run may fetch.
    pub page_limit: u32,
}

/// The canonical [`BenchmarkError::LiveDisabled`] refusal for a target.
pub(crate) fn live_disabled(target: CompetitorTarget) -> BenchmarkError {
    BenchmarkError::LiveDisabled {
        provider: target.provider_name(),
        env_var: target.env_var(),
    }
}

/// Shared adapter preflight: normalize the API key, enforce the fail-closed
/// gate, and return the trimmed key ready for header construction.
///
/// # Errors
///
/// [`BenchmarkError::LiveDisabled`] for a missing/blank key or a missing
/// explicit opt-in flag.
pub(crate) fn resolve_key_and_gate(
    target: CompetitorTarget,
    api_key: Option<&str>,
    opt_in_flag: bool,
) -> Result<String> {
    let key = match api_key.map(str::trim).filter(|key| !key.is_empty()) {
        Some(key) => key.to_string(),
        None => return Err(live_disabled(target)),
    };
    check_live_gate(target, true, opt_in_flag)?;
    Ok(key)
}
