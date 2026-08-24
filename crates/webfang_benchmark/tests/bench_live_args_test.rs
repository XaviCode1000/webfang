//! Slice-2 `bench_live` argument validation (NFR-4): the CLI must refuse
//! anything but an explicit `--target <firecrawl|crawl4ai>` plus at most the
//! explicit opt-in flag, and must expose the parsed decision as data so the
//! fail-closed gate can act on it.
//!
//! Run: cargo nextest run -p webfang_benchmark --test bench_live_args_test

use webfang_benchmark::competitor::{self, CompetitorTarget};
use webfang_benchmark::BenchmarkError;

fn args<'a>(list: &'a [&'a str]) -> impl Iterator<Item = String> + 'a {
    list.iter().map(|s| (*s).to_string())
}

/// Full happy path: target + opt-in flag.
#[test]
fn parses_target_and_opt_in_flag() {
    let parsed =
        competitor::parse_bench_live_args(args(&["--target", "firecrawl", "--i-understand-costs"]))
            .expect("valid invocation parses");
    assert_eq!(parsed.target, CompetitorTarget::Firecrawl);
    assert!(parsed.opt_in);
}

/// Opt-in flag is optional at the parse level (the gate still requires it).
#[test]
fn opt_in_flag_is_optional_at_parse_level() {
    let parsed = competitor::parse_bench_live_args(args(&["--target", "crawl4ai"]))
        .expect("valid invocation parses");
    assert_eq!(parsed.target, CompetitorTarget::Crawl4Ai);
    assert!(!parsed.opt_in);
}

/// Missing --target is a usage error.
#[test]
fn missing_target_is_usage_error() {
    let err = competitor::parse_bench_live_args(args(&[])).expect_err("must fail");
    let rendered = err.to_string();
    assert!(
        rendered.contains("--target") && rendered.contains("--i-understand-costs"),
        "usage text must name both flags, got: {rendered}"
    );
}

/// Unknown target values surface as typed errors naming the valid choices.
#[test]
fn unknown_target_value_is_typed_error() {
    let err = competitor::parse_bench_live_args(args(&["--target", "scrapingbee"]))
        .expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
}

/// Unknown flags are rejected outright (fail-closed CLI posture).
#[test]
fn unexpected_argument_is_rejected() {
    let err = competitor::parse_bench_live_args(args(&["--target", "firecrawl", "--yolo"]))
        .expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
}

/// --target without a value is a usage error.
#[test]
fn target_without_value_is_usage_error() {
    let err = competitor::parse_bench_live_args(args(&["--target"])).expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
}

/// Repeated --target is rejected instead of silently last-wins.
#[test]
fn repeated_target_is_rejected() {
    let err =
        competitor::parse_bench_live_args(args(&["--target", "firecrawl", "--target", "crawl4ai"]))
            .expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
}
