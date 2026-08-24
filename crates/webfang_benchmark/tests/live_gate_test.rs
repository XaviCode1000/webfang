//! Slice-2 live-run gate logic (NFR-4, C-3): Tier B execution is a binary-only,
//! fail-closed path. A live run requires BOTH a non-empty provider API key in
//! the environment AND an explicit CLI opt-in flag; anything less is the typed
//! [`BenchmarkError::LiveDisabled`].
//!
//! The gate itself is pure (key presence passed in as a bool) so it is fully
//! unit-testable without mutating process environment state.
//!
//! Run: cargo nextest run -p webfang_benchmark --test live_gate_test

use webfang_benchmark::competitor::{self, CompetitorTarget};
use webfang_benchmark::BenchmarkError;

/// Target names parse exactly as documented in bench_live usage.
#[test]
fn target_names_parse_from_cli_strings() {
    assert_eq!(
        CompetitorTarget::parse_name("firecrawl").expect("firecrawl parses"),
        CompetitorTarget::Firecrawl
    );
    assert_eq!(
        CompetitorTarget::parse_name("crawl4ai").expect("crawl4ai parses"),
        CompetitorTarget::Crawl4Ai
    );
}

/// Unknown target names are typed usage errors, never panics.
#[test]
fn unknown_target_name_is_typed_error() {
    let err = CompetitorTarget::parse_name("scrapingbee").expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
}

/// Each target declares its own provider name and env var.
#[test]
fn targets_declare_provider_and_env_var() {
    assert_eq!(CompetitorTarget::Firecrawl.provider_name(), "firecrawl");
    assert_eq!(CompetitorTarget::Firecrawl.env_var(), "FIRECRAWL_API_KEY");
    assert_eq!(CompetitorTarget::Crawl4Ai.provider_name(), "crawl4ai");
    assert_eq!(CompetitorTarget::Crawl4Ai.env_var(), "CRAWL4AI_API_KEY");
}

/// Missing env key: refused even with the opt-in flag.
#[test]
fn missing_key_refuses_even_with_opt_in() {
    let err = competitor::check_live_gate(CompetitorTarget::Firecrawl, false, true)
        .expect_err("must refuse without key");
    match err {
        BenchmarkError::LiveDisabled { provider, env_var } => {
            assert_eq!(provider, "firecrawl");
            assert_eq!(env_var, "FIRECRAWL_API_KEY");
            let rendered = err.to_string();
            assert!(
                rendered.contains("FIRECRAWL_API_KEY") && rendered.contains("--i-understand-costs"),
                "refusal must name the env var and the opt-in flag, got: {rendered}"
            );
        },
        other => panic!("expected LiveDisabled, got {other:?}"),
    }
}

/// Env key present but no explicit opt-in flag: still refused.
#[test]
fn opt_in_flag_is_required_even_with_key() {
    let err = competitor::check_live_gate(CompetitorTarget::Crawl4Ai, true, false)
        .expect_err("must refuse without opt-in");
    assert!(matches!(err, BenchmarkError::LiveDisabled { .. }));
}

/// Both conditions satisfied: the gate opens.
#[test]
fn key_plus_opt_in_opens_the_gate() {
    competitor::check_live_gate(CompetitorTarget::Firecrawl, true, true)
        .expect("gate must open for key + opt-in");
}
