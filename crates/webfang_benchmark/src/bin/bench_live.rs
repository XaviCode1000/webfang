//! Tier B live-competitor binary (slice 2, design §6).
//!
//! Offline skeleton: parses/validates arguments, enforces the fail-closed
//! live-run gate (non-empty provider API key AND explicit `--i-understand-costs`
//! opt-in), prepares the competitor request, and reports the typed refusal or
//! execution-deferral outcome. NO network call exists in this build — when
//! Tier B execution lands, adapters will use wreq exclusively (C-3).
//!
//! Usage: `bench_live --target <firecrawl|crawl4ai> [--i-understand-costs]`
//!
//! Exit codes: success never happens in this build (execution is deferred by
//! the adapters); refusals/failures print the typed error to stderr and exit 1.

use std::process::ExitCode;

use webfang_benchmark::competitor::{
    self, CompetitorTarget, Crawl4AiConfig, FirecrawlConfig, StartCrawlParams,
};
use webfang_benchmark::error::Result;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench_live refused: {error}");
            ExitCode::FAILURE
        },
    }
}

fn run() -> Result<()> {
    let parsed = competitor::parse_bench_live_args(std::env::args().skip(1))?;

    // Fail-closed gate: env key presence + explicit CLI opt-in. Nothing below
    // executes unless both hold.
    competitor::evaluate_live_gate(parsed.target, parsed.opt_in)?;

    // Placeholder target until the Tier B sampling slice wires real site lists;
    // its validation is fully implemented in the adapters and fails loudly.
    let params = StartCrawlParams {
        target_url: String::new(),
        page_limit: 0,
    };
    let api_key = std::env::var(parsed.target.env_var()).ok();

    let outcome = match parsed.target {
        CompetitorTarget::Firecrawl => block_on(competitor::firecrawl::run(
            &FirecrawlConfig::default(),
            &params,
            api_key.as_deref(),
            parsed.opt_in,
        )),
        CompetitorTarget::Crawl4Ai => block_on(competitor::crawl4ai::run(
            &Crawl4AiConfig::default(),
            &params,
            api_key.as_deref(),
            parsed.opt_in,
        )),
    }?;

    println!("live run completed");
    drop(outcome);
    Ok(())
}

/// Drive one future on a dedicated current-thread runtime (bench_tier_a
/// precedent); the harness owns its runtimes, never a global one.
fn block_on<F: std::future::Future>(future: F) -> Result<F::Output> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(rt.block_on(future))
}
