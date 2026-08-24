//! Tier B live-run corpus manifest v2: the ordered site list a full pass
//! crawls through the Firecrawl free plan, with per-site page caps and a
//! credit projection.
//!
//! Methodology categories (each site declares exactly one):
//! - [`SiteCategory::Sandboxed`]: ethical scraping grounds built for crawler
//!   practice — the only places that explicitly consent to being crawled.
//! - [`SiteCategory::StaticReal`]: real-world static sites, no WAF.
//! - [`SiteCategory::WafProtected`]: Cloudflare/WAF-protected content.
//! - [`SiteCategory::HeavyDynamic`]: heavy dynamic/auth-walled sites.
//! - [`SiteCategory::OwnerPick`]: reserved owner slots (OPEN-2) for sites
//!   where the owner personally hit blocks; empty until assigned.
//!
//! # Egress type (methodology caveat)
//!
//! Cloudflare challenge passage depends on IP quality far more than on the
//! browser engine under test. Every live report header MUST print the egress
//! type from [`EGRESS_TYPE_ENV_VAR`] (e.g. `residential`, `datacenter`,
//! `residential-proxy`); an undocumented egress makes the numbers
//! non-reproducible and must be labeled as such.
//!
//! This module is pure data + pure functions: nothing here performs I/O.

/// Environment variable carrying the egress type of a live run
/// (`residential` | `datacenter` | `residential-proxy`).
pub const EGRESS_TYPE_ENV_VAR: &str = "WEBFANG_BENCH_EGRESS_TYPE";

/// Default live-pass credit budget guard (`--max-credits`) sized against the
/// free plan's 1000 monthly credits: calibration (~30) + full pass (~150) +
/// retry margin.
pub const DEFAULT_MAX_CREDITS: u32 = 250;

/// Free-plan rate assumption for page-fetch actions (single source:
/// [`crate::cost::config::FreeTierPlan::scrape_crawl_map_cost_per_page`]).
pub const CREDITS_PER_PAGE: f64 = 1.0;

/// Methodology category of a corpus site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteCategory {
    /// Ethical scraping sandbox built for crawler practice.
    Sandboxed,
    /// Real-world static site without WAF protection.
    StaticReal,
    /// Cloudflare/WAF-protected content site.
    WafProtected,
    /// Heavy dynamic / auth-walled site.
    HeavyDynamic,
    /// Reserved owner-pick slot; stays empty until the owner assigns it.
    OwnerPick,
}

/// One corpus entry: host plus its per-pass page cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierBSite {
    /// Site hostname (scheme-independent).
    pub host: &'static str,
    /// Methodology category used in report grouping.
    pub category: SiteCategory,
    /// Maximum pages this site contributes per pass (0 = unassigned slot).
    pub page_cap: u32,
}

/// The ordered Tier B corpus manifest v2 (~147 pages/pass at current caps).
///
/// Order is meaningful: sandboxes first (safe warm-up), then static controls,
/// then WAF-protected, then heavy sites last so a mid-run failure biases the
/// cheapest-to-rerun prefix.
pub const TIER_B_CORPUS: &[TierBSite] = &[
    // Category A — sandboxes (ethical scraping grounds)
    TierBSite {
        host: "scrapeme.dev",
        category: SiteCategory::Sandboxed,
        page_cap: 20,
    },
    TierBSite {
        host: "web-scraping.dev",
        category: SiteCategory::Sandboxed,
        page_cap: 15,
    },
    TierBSite {
        host: "qscrape.dev",
        category: SiteCategory::Sandboxed,
        page_cap: 12,
    },
    // Category B — static real sites (no WAF controls)
    TierBSite {
        host: "rust-lang.org",
        category: SiteCategory::StaticReal,
        page_cap: 10,
    },
    TierBSite {
        host: "news.ycombinator.com",
        category: SiteCategory::StaticReal,
        page_cap: 10,
    },
    // Category C — WAF/Cloudflare-protected
    TierBSite {
        host: "blog.cloudflare.com",
        category: SiteCategory::WafProtected,
        page_cap: 15,
    },
    TierBSite {
        host: "developers.cloudflare.com",
        category: SiteCategory::WafProtected,
        page_cap: 15,
    },
    TierBSite {
        host: "web.dev",
        category: SiteCategory::WafProtected,
        page_cap: 15,
    },
    // Category D — heavy dynamic/auth-walled
    TierBSite {
        host: "medium.com",
        category: SiteCategory::HeavyDynamic,
        page_cap: 20,
    },
    TierBSite {
        host: "gitlab.com",
        category: SiteCategory::HeavyDynamic,
        page_cap: 15,
    },
    // Category E — four RESERVED owner slots (OPEN-2): fill host + cap later;
    // zero caps keep them inert until assigned.
    TierBSite {
        host: "",
        category: SiteCategory::OwnerPick,
        page_cap: 0,
    },
    TierBSite {
        host: "",
        category: SiteCategory::OwnerPick,
        page_cap: 0,
    },
    TierBSite {
        host: "",
        category: SiteCategory::OwnerPick,
        page_cap: 0,
    },
    TierBSite {
        host: "",
        category: SiteCategory::OwnerPick,
        page_cap: 0,
    },
];

/// Total pages a full pass attempts over `corpus`.
#[must_use]
pub fn total_pages(corpus: &[TierBSite]) -> u32 {
    corpus.iter().map(|site| site.page_cap).sum()
}

/// Projected credit spend for `total_pages` at the free-plan page rate.
///
/// Pure arithmetic over declared constants — no I/O, no request building.
#[must_use]
pub fn projected_credits(total_pages: u32) -> f64 {
    f64::from(total_pages) * CREDITS_PER_PAGE
}

/// Read the documented egress type from the environment.
///
/// `None` means undocumented egress: report headers must label the numbers
/// non-reproducible (Cloudflare passage depends on IP quality more than on
/// the browser engine).
#[must_use]
pub fn egress_type_from_env() -> Option<String> {
    std::env::var(EGRESS_TYPE_ENV_VAR)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
