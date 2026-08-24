//! THE single pricing/assumption section (ADR-B5, AC-4.2).
//!
//! ALL pricing constants live here — update prices ONLY in this module. Every
//! constant carries its source URL and retrieval date; those strings are
//! rendered VERBATIM into every report header (AC-4.1). Competitor pricing is
//! populated in slice 2, in this same section only.

/// WebFang self-host infrastructure basis for the $/1k-pages formula.
#[derive(Debug, Clone)]
pub struct InfraAssumptions {
    /// Instance list price per hour (USD).
    pub instance_hourly_usd: f64,
    /// Instance RAM (GiB) used to amortize the static RAM proxy.
    pub instance_ram_gb: f64,
    /// Where the price came from (printed verbatim in report headers).
    pub source_url: &'static str,
    /// ISO retrieval date for `source_url` (printed verbatim).
    pub retrieved: &'static str,
}

/// Firecrawl tier pricing shape (populated with real figures in slice 2).
#[derive(Debug, Clone)]
pub struct FirecrawlPricing {
    /// Public credit-tier prices (USD): the documented 16 / 83 / 333 tiers.
    pub tier_usd: [f64; 3],
    /// Assumed pages-per-credit yield under our workload.
    pub credits_per_page_assumption: f64,
    /// Source URL + retrieval date (verbatim into report headers).
    pub source_url: &'static str,
    pub retrieved: &'static str,
}

/// Crawl4AI documented self-host sizing (populated in slice 2).
#[derive(Debug, Clone)]
pub struct Crawl4AiSizing {
    /// Self-host instance hourly cost (USD) from documented sizing guidance.
    pub instance_hourly_usd: f64,
    /// Self-host instance RAM (GiB).
    pub instance_ram_gb: f64,
    /// Source URL + retrieval date (verbatim into report headers).
    pub source_url: &'static str,
    pub retrieved: &'static str,
}

/// The one configuration object every cost figure dereferences.
///
/// Slice-1 defaults are declared assumptions, challengeable by construction:
/// they appear verbatim in the report header so readers can contest them.
#[derive(Debug, Clone)]
pub struct CostConfig {
    /// WebFang self-host basis.
    pub infra: InfraAssumptions,
    /// Competitor mapping (slice 2 renders real cells; slice 1 placeholders).
    pub firecrawl: FirecrawlPricing,
    /// Competitor mapping (slice 2).
    pub crawl4ai: Crawl4AiSizing,
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            infra: InfraAssumptions {
                // Declared assumption: single small VPS class, list price.
                instance_hourly_usd: 0.02,
                instance_ram_gb: 4.0,
                source_url: "https://www.hetzner.com/cloud#pricing",
                retrieved: "2026-08-24",
            },
            firecrawl: FirecrawlPricing {
                // Documented public credit tiers; page-yield assumption declared.
                tier_usd: [16.0, 83.0, 333.0],
                credits_per_page_assumption: 1.0,
                source_url: "https://www.firecrawl.dev/pricing",
                retrieved: "2026-08-24",
            },
            crawl4ai: Crawl4AiSizing {
                // Declared assumption pending slice-2 documentation pass.
                instance_hourly_usd: 0.02,
                instance_ram_gb: 4.0,
                source_url: "https://docs.crawl4ai.com/core/installation/",
                retrieved: "2026-08-24",
            },
        }
    }
}
