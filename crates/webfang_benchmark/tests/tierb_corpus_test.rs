//! Tier B live-run corpus manifest v2: ordered site list with per-site page
//! caps and credit projection. Pure data + pure functions — fully offline.
//!
//! Run: cargo nextest run -p webfang_benchmark --test tierb_corpus_test

use webfang_benchmark::competitor::tierb_corpus::{
    self, SiteCategory, EGRESS_TYPE_ENV_VAR,
};

/// The manifest totals ~147 pages/pass: 147 from real sites plus four
/// reserved owner slots contributing 0 pages until the owner fills them
/// (OPEN-2).
#[test]
fn corpus_totals_pinned_at_147_pages() {
    let total = tierb_corpus::total_pages(tierb_corpus::TIER_B_CORPUS);
    assert_eq!(total, 147, "corpus page caps must sum to 147");
}

/// Credit projection at the free-plan rate (1 credit/page) stays well under
/// the 250-credit default budget guard.
#[test]
fn projected_credits_fit_default_budget() {
    let total = tierb_corpus::total_pages(tierb_corpus::TIER_B_CORPUS);
    let credits = tierb_corpus::projected_credits(total);
    assert!((credits - 147.0).abs() < 1e-9);
    assert!(
        credits <= tierb_corpus::DEFAULT_MAX_CREDITS as f64,
        "full pass ({credits}) must fit the default budget"
    );
}

/// Exactly four owner-pick slots exist and none of them contributes pages
/// while unassigned.
#[test]
fn owner_pick_slots_are_reserved_and_empty() {
    let picks: Vec<_> = tierb_corpus::TIER_B_CORPUS
        .iter()
        .filter(|site| site.category == SiteCategory::OwnerPick)
        .collect();
    assert_eq!(picks.len(), 4, "four reserved owner slots expected");
    assert!(
        picks.iter().all(|site| site.page_cap == 0),
        "unassigned owner slots contribute zero pages"
    );
}

/// Every category of the methodology matrix is represented by at least one
/// real site (owner-pick aside): sandboxes, static controls, WAF-protected,
/// heavy dynamic.
#[test]
fn every_methodology_category_is_represented() {
    for category in [
        SiteCategory::Sandboxed,
        SiteCategory::StaticReal,
        SiteCategory::WafProtected,
        SiteCategory::HeavyDynamic,
    ] {
        assert!(
            tierb_corpus::TIER_B_CORPUS
                .iter()
                .any(|site| site.category == category),
            "category {category:?} missing from the Tier B corpus"
        );
    }
}

/// Egress type is read from the documented env var; unset/blank means
/// undocumented egress (non-reproducible numbers) and yields `None`.
#[test]
fn egress_type_reads_env_var() {
    // SAFETY-free on edition 2021; this test owns WEBFANG_BENCH_EGRESS_TYPE.
    std::env::set_var(EGRESS_TYPE_ENV_VAR, "residential-proxy");
    assert_eq!(
        tierb_corpus::egress_type_from_env().as_deref(),
        Some("residential-proxy")
    );
    std::env::set_var(EGRESS_TYPE_ENV_VAR, "   ");
    assert_eq!(tierb_corpus::egress_type_from_env(), None);
    std::env::remove_var(EGRESS_TYPE_ENV_VAR);
    assert_eq!(tierb_corpus::egress_type_from_env(), None);
}
