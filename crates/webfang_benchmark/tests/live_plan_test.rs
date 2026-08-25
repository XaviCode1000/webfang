//! Tier B Step 0 live-run guards: projected-credit budget refusal, free-plan
//! concurrency clamp, and inter-request pacing. All pure library-level logic
//! (unit-testable without running the bin); nothing here performs I/O.
//!
//! Run: cargo nextest run -p webfang_benchmark --test live_plan_test

use webfang_benchmark::competitor::{
    plan_live_run, CompetitorTarget, DEFAULT_INTER_REQUEST_DELAY_MS, DEFAULT_MAX_CREDITS,
    FIRECRAWL_MAX_CONCURRENCY,
};
use webfang_benchmark::BenchmarkError;

/// Planning that would exceed the credit budget is a typed refusal issued
/// BEFORE any request is prepared or sent.
#[test]
fn budget_overflow_is_typed_refusal_before_any_request() {
    let err = plan_live_run(CompetitorTarget::Firecrawl, 300, 1, 250, 1.0)
        .expect_err("300 projected credits must exceed the 250 budget");
    match &err {
        BenchmarkError::BudgetExceeded {
            provider,
            projected_credits,
            budget,
        } => {
            assert_eq!(*provider, "firecrawl");
            assert!((projected_credits - 300.0).abs() < 1e-9);
            assert_eq!(*budget, 250);
        },
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }
    let rendered = err.to_string();
    assert!(
        rendered.contains("before any request"),
        "refusal must state nothing was executed, got: {rendered}"
    );
}

/// Exactly-at-budget plans are accepted (<=, not <).
#[test]
fn budget_boundary_is_inclusive() {
    let plan =
        plan_live_run(CompetitorTarget::Firecrawl, 147, 1, 147, 1.0).expect("at-budget plan");
    assert!((plan.projected_credits - 147.0).abs() < 1e-9);
}

/// Any requested concurrency above the Firecrawl free-plan ceiling of 2 is
/// hard-clamped down to 2.
#[test]
fn firecrawl_concurrency_clamped_to_two() {
    let clamped =
        plan_live_run(CompetitorTarget::Firecrawl, 10, 8, DEFAULT_MAX_CREDITS, 1.0).expect("plan");
    assert_eq!(clamped.requested_concurrency, 8);
    assert_eq!(clamped.concurrency, FIRECRAWL_MAX_CONCURRENCY);
    assert_eq!(clamped.concurrency, 2);

    let modest =
        plan_live_run(CompetitorTarget::Firecrawl, 10, 2, DEFAULT_MAX_CREDITS, 1.0).expect("plan");
    assert_eq!(modest.concurrency, 2);

    let minimal =
        plan_live_run(CompetitorTarget::Firecrawl, 10, 1, DEFAULT_MAX_CREDITS, 1.0).expect("plan");
    assert_eq!(minimal.concurrency, 1);
}

/// The clamp is a Firecrawl free-plan rule: Crawl4AI (self-hosted) keeps its
/// requested concurrency.
#[test]
fn crawl4ai_concurrency_is_not_clamped() {
    let plan =
        plan_live_run(CompetitorTarget::Crawl4Ai, 10, 8, DEFAULT_MAX_CREDITS, 0.0).expect("plan");
    assert_eq!(plan.concurrency, 8);
}

/// Every plan carries an inter-request delay so pacing is part of the printed
/// plan output, never an implicit afterthought.
#[test]
fn plan_output_carries_inter_request_delay() {
    let plan =
        plan_live_run(CompetitorTarget::Firecrawl, 10, 1, DEFAULT_MAX_CREDITS, 1.0).expect("plan");
    assert_eq!(plan.delay_ms, DEFAULT_INTER_REQUEST_DELAY_MS);
    assert!(plan.delay_ms > 0, "delay_ms must be present and positive");
}

/// Default budget guard matches the approved live-run plan (~250 of 1000
/// monthly credits).
#[test]
fn default_budget_is_250_credits() {
    assert_eq!(DEFAULT_MAX_CREDITS, 250);
}
