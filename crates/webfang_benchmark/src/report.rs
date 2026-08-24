//! Markdown report generation (FR-5).
//!
//! Renders the comparison table (one row per dimension × strategy) plus the
//! verbatim assumptions header (AC-4.1) and the ADR-B4 two-block layout:
//! deterministic quantities (Block A) stay outside the sentinels; volatile
//! wall-clock quantities (p50/p95, pages/sec, wall-clock) are wrapped in
//! exactly one pair of `<!-- volatile -->` sentinels (Block B). The generator
//! NEVER emits temp paths, ports, hostnames, or wall-clock timestamps (AC-5.2);
//! run dates come only from an explicit caller-supplied `--as-of` value.
//!
//! Tier labeling is enforced by type: rows are built from [`JsStrategy`] via
//! [`tier_label`], never from free strings.

use crate::aggregate::{ErrorBuckets, StrategyMetrics};
use crate::cost::CostConfig;
use crate::error::Result;
use std::fmt::Write as _;
use webfang_core::domain::JsStrategy;

/// Newline written through `write!` without tripping the
/// `write_with_newline` lint (we build rows incrementally).
const NL: char = '\n';

/// The honest Tier A label (AC-5.2): every row produced from this harness
/// measures the simulated challenge corpus, nothing else.
pub const TIER_A_LABEL: &str = "simulated challenge corpus";

/// Tier label enforced by type: a strategy value, not an arbitrary string,
/// is the only way to produce a row's tier cell.
#[must_use]
pub fn tier_label(strategy: JsStrategy) -> &'static str {
    match strategy {
        JsStrategy::Static | JsStrategy::Hybrid | JsStrategy::Full => TIER_A_LABEL,
    }
}

/// Render the full Markdown report.
///
/// # Errors
///
/// - [`crate::error::BenchmarkError::Render`] if formatting to the string
///   buffer fails (cannot happen for `String` writes in practice, kept for the
///   crate-wide typed-error contract).
pub fn render(metrics: &[StrategyMetrics], config: &CostConfig) -> Result<String> {
    let mut out = String::new();

    write_header(&mut out, config)?;
    write_methodology(&mut out)?;
    // Block B (volatile) opens here and closes after the timing table — the
    // ONLY place volatile quantities may appear (ADR-B4).
    writeln!(out, "<!-- volatile -->")
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    write_timing_table(&mut out, metrics)?;
    writeln!(out, "<!-- volatile -->")
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    write_deterministic_table(&mut out, metrics)?;

    Ok(out)
}

/// Verbatim assumptions header (AC-4.1): every source URL + retrieval date
/// from [`CostConfig`] appears exactly as stored in `cost/config.rs`.
fn write_header(out: &mut String, config: &CostConfig) -> Result<()> {
    writeln!(out, "# WebFang Benchmark Report")
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(out).map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(out, "## Assumptions & sources")
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(
        out,
        "Formula: $/1k = hourly x (1000 / pages_per_sec / 3600) + hourly x (ram_bytes / (ram_gb * 2^30))"
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(
        out,
        "- Infra basis: {} (retrieved {}) — ${:.2}/h, {:.1} GiB RAM",
        config.infra.source_url,
        config.infra.retrieved,
        config.infra.instance_hourly_usd,
        config.infra.instance_ram_gb
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(
        out,
        "- Firecrawl pricing: {} (retrieved {}) — credit tiers ${:.0}/${:.0}/${:.0} for {:.0}/{:.0}/{:.0} credits, assumption {:.2} pages/credit",
        config.firecrawl.source_url, config.firecrawl.retrieved,
        config.firecrawl.tier_usd[0], config.firecrawl.tier_usd[1],
        config.firecrawl.tier_usd[2], config.firecrawl.tier_credits[0],
        config.firecrawl.tier_credits[1], config.firecrawl.tier_credits[2],
        config.firecrawl.credits_per_page_assumption
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(
        out,
        "- Crawl4AI sizing: {} (retrieved {}) — ${:.2}/h, {:.1} GiB RAM ({})",
        config.crawl4ai.source_url,
        config.crawl4ai.retrieved,
        config.crawl4ai.instance_hourly_usd,
        config.crawl4ai.instance_ram_gb,
        config.crawl4ai.sizing_note
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    Ok(())
}

/// Methodology notes (AC-4.1/FR-5): D2 limitation + percentile convention.
fn write_methodology(out: &mut String) -> Result<()> {
    writeln!(out).map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(out, "## Methodology notes")
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(
        out,
        "- D2 limitation: no per-layer latency split in v1; per-strategy deltas are the attribution proxy."
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(
        out,
        "- Percentiles use the nearest-rank convention (idx = ceil(p/100 x n), 1-based, no interpolation)."
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(
        out,
        "- Wall-clock quantities are volatile by nature and excluded from byte-compared output."
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    writeln!(out).map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    Ok(())
}

/// Block B: volatile wall-clock quantities (ADR-B4), inside the sentinels.
fn write_timing_table(out: &mut String, metrics: &[StrategyMetrics]) -> Result<()> {
    writeln!(
        out,
        "## Timing (wall-clock dependent — excluded from byte-compare)"
    )
    .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    header_row(
        out,
        [
            "Strategy",
            "Tier",
            "p50 ms",
            "p95 ms",
            "pages/sec",
            "wall-clock secs",
        ],
    )?;
    for m in metrics {
        writeln!(
            out,
            "| {:?} | {} | {:.2} | {:.2} | {:.2} | {:.4} |",
            m.strategy,
            tier_label(m.strategy),
            m.p50_ms,
            m.p95_ms,
            m.pages_per_sec,
            m.wall_clock_secs
        )
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    }
    writeln!(out).map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    Ok(())
}

/// Block A: deterministic quantities — counts and derived integers only.
fn write_deterministic_table(out: &mut String, metrics: &[StrategyMetrics]) -> Result<()> {
    writeln!(out, "## Results (deterministic counts)")
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    header_row(
        out,
        [
            "Strategy",
            "Tier",
            "success rate",
            "WAF",
            "HTTP",
            "timeout",
            "network",
            "rate limit",
            "extraction",
            "internal",
            "panic",
            "RAM bytes",
        ],
    )?;
    for m in metrics {
        let ErrorBuckets {
            waf,
            http,
            timeout,
            network,
            rate_limit,
            extraction,
            internal,
            panic,
        } = m.error_buckets;
        writeln!(
            out,
            "| {:?} | {} | {:.4} | {waf} | {http} | {timeout} | {network} | {rate_limit} | {extraction} | {internal} | {panic} | {} |",
            m.strategy,
            tier_label(m.strategy),
            m.success_rate,
            m.ram_cost_bytes
        )
        .map_err(|e| crate::error::BenchmarkError::Render(e.to_string()))?;
    }
    Ok(())
}

fn header_row<const N: usize>(out: &mut String, cols: [&str; N]) -> Result<()> {
    let _ = write!(out, "| {}", cols[0]);
    for col in &cols[1..] {
        let _ = write!(out, " | {col}");
    }
    let _ = write!(out, " |{NL}");
    let _ = write!(out, "|");
    for _ in cols {
        let _ = write!(out, " --- |");
    }
    let _ = write!(out, "{NL}");
    Ok(())
}
