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

/// Firecrawl public credit-tier pricing.
#[derive(Debug, Clone)]
pub struct FirecrawlPricing {
    /// Public credit-tier prices (USD): the documented 16 / 83 / 333 tiers
    /// (Hobby / Standard / Growth).
    pub tier_usd: [f64; 3],
    /// Credit allotment per tier (Hobby 5k / Standard 100k / Growth 500k),
    /// from the same pricing page as [`Self::tier_usd`].
    pub tier_credits: [f64; 3],
    /// Assumed pages-per-credit yield under our workload.
    pub credits_per_page_assumption: f64,
    /// Source URL + retrieval date (verbatim into report headers).
    pub source_url: &'static str,
    pub retrieved: &'static str,
}

/// Crawl4AI documented self-host sizing.
#[derive(Debug, Clone)]
pub struct Crawl4AiSizing {
    /// Self-host instance hourly cost (USD) from documented sizing guidance.
    pub instance_hourly_usd: f64,
    /// Self-host instance RAM (GiB).
    pub instance_ram_gb: f64,
    /// Human-readable sizing assumption rendered verbatim into report
    /// headers so readers can contest it.
    pub sizing_note: &'static str,
    /// Source URL + retrieval date (verbatim into report headers).
    pub source_url: &'static str,
    pub retrieved: &'static str,
}

/// Firecrawl FREE-tier plan limits and economics (single-source, AC-4.2).
///
/// Captures the documented free-plan constraints that gate Tier B live runs:
/// monthly credit allowance, hard concurrency ceiling, per-action credit
/// costs, and the no-rollover / no-charge-for-failures rules. The Hobby tier
/// price/credit pair provides a shadow price translating free-plan usage into
/// paid-equivalent dollars.
#[derive(Debug, Clone)]
pub struct FreeTierPlan {
    /// Monthly free credits granted by the plan.
    pub credits_monthly: f64,
    /// Hard maximum concurrent requests allowed by the plan.
    pub max_concurrent: u32,
    /// Whether unused credits roll over to the next month.
    pub rollover: bool,
    /// Credit cost per page for scrape/crawl/map actions.
    pub scrape_crawl_map_cost_per_page: f64,
    /// Credit cost per 10 search results.
    pub search_cost_per_10_results: f64,
    /// Credit cost per minute of browser interact time.
    pub interact_cost_per_minute: f64,
    /// Whether failed requests are still charged credits.
    pub failed_requests_charged: bool,
    /// Hobby-tier monthly price used as the shadow-price numerator (USD).
    pub shadow_tier_usd: f64,
    /// Hobby-tier credit allotment used as the shadow-price denominator.
    pub shadow_tier_credits: f64,
    /// Source URL + retrieval date (verbatim into report headers).
    pub source_url: &'static str,
    pub retrieved: &'static str,
}

impl Default for FreeTierPlan {
    fn default() -> Self {
        Self {
            credits_monthly: 1000.0,
            max_concurrent: 2,
            rollover: false,
            scrape_crawl_map_cost_per_page: 1.0,
            search_cost_per_10_results: 2.0,
            interact_cost_per_minute: 2.0,
            failed_requests_charged: false,
            // Shadow-price basis: Hobby tier $16 / 5000 credits = $0.0032 per
            // credit; lets free-plan results be read as paid economics.
            shadow_tier_usd: 16.0,
            shadow_tier_credits: 5000.0,
            source_url: "https://www.firecrawl.dev/pricing",
            retrieved: "2026-08-24",
        }
    }
}

impl FreeTierPlan {
    /// Paid-equivalent price per credit derived from the Hobby tier basis.
    #[must_use]
    pub fn shadow_price_usd_per_credit(&self) -> f64 {
        self.shadow_tier_usd / self.shadow_tier_credits
    }
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
                // Documented public credit tiers (Hobby/Standard/Growth) with
                // their credit allotments; page-yield assumption declared.
                tier_usd: [16.0, 83.0, 333.0],
                tier_credits: [5_000.0, 100_000.0, 500_000.0],
                credits_per_page_assumption: 1.0,
                source_url: "https://www.firecrawl.dev/pricing",
                retrieved: "2026-08-24",
            },
            crawl4ai: Crawl4AiSizing {
                // Declared assumption: single self-hosted VPS-class instance
                // (2 vCPU / 4 GiB RAM), list-price hourly equivalent.
                instance_hourly_usd: 0.02,
                instance_ram_gb: 4.0,
                sizing_note: "single self-hosted instance, 2 vCPU / 4 GiB RAM (VPS class)",
                source_url: "https://docs.crawl4ai.com/core/installation/",
                retrieved: "2026-08-24",
            },
        }
    }
}
