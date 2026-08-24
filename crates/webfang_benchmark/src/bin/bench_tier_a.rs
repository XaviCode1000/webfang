//! Tier A benchmark binary (slice 1, design §1/§3 data flow).
//!
//! Runs the full in-process pipeline against the simulated-WAF corpus:
//! `corpus::serve() → runner::run_all → aggregate → cost::estimate →
//! report::render` and writes the result to `benchmark-report.md`.
//!
//! Usage: `bench_tier_a [--as-of YYYY-MM-DD]`
//!
//! `--as-of` stamps the report with a caller-supplied date string rendered
//! verbatim above the generated body; when omitted, no date appears anywhere
//! in the output (design §7: the generator never emits wall-clock timestamps).
//!
//! Note: `runner::run_all` builds its own current-thread runtimes internally,
//! so this binary calls it off any ambient runtime — only `corpus::serve()`
//! needs an async context, provided by the local runtime below.

use std::process::ExitCode;

use webfang_benchmark::corpus;
use webfang_benchmark::cost::{self, CostConfig};
use webfang_benchmark::error::{BenchmarkError, Result};
use webfang_benchmark::report;
use webfang_benchmark::runner;

const USAGE: &str = "usage: bench_tier_a [--as-of YYYY-MM-DD]";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench_tier_a failed: {error}");
            ExitCode::FAILURE
        },
    }
}

fn run() -> Result<()> {
    let as_of = parse_args(std::env::args().skip(1))?;

    // Local runtime solely for corpus startup; run_all is synchronous and owns
    // its own per-run current-thread runtimes (design §2).
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let handle = rt.block_on(corpus::serve())?;

    let outcomes = runner::run_all(&handle.manifest)?;
    let config = CostConfig::default();

    for outcome in &outcomes {
        cost::estimate(&outcome.metrics, &config)?;
    }

    let metrics: Vec<_> = outcomes.iter().map(|o| o.metrics.clone()).collect();
    let mut markdown = report::render(&metrics, &config)?;
    if let Some(date) = as_of {
        markdown = format!("Run date (caller-supplied): {date}\n\n{markdown}");
    }

    std::fs::write("benchmark-report.md", markdown)?;
    println!("wrote benchmark-report.md");
    Ok(())
}

/// Minimal arg parsing: at most one `--as-of <value>`; anything else is a
/// usage error surfaced as a typed [`BenchmarkError`] (no panics outside tests).
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<String>> {
    let mut iter = args.peekable();
    let mut as_of = None;
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--as-of" => {
                let value = iter.next().ok_or_else(|| {
                    BenchmarkError::Render(format!("{USAGE}: --as-of needs a value"))
                })?;
                if value.starts_with('-') {
                    return Err(BenchmarkError::Render(format!(
                        "{USAGE}: --as-of needs a date value"
                    )));
                }
                as_of = Some(value);
            },
            other => {
                return Err(BenchmarkError::Render(format!(
                    "{USAGE}: unexpected argument `{other}`"
                )))
            },
        }
    }
    Ok(as_of)
}
