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

/// --max-credits overrides the default budget guard.
#[test]
fn max_credits_flag_is_parsed() {
    let parsed =
        competitor::parse_bench_live_args(args(&["--target", "firecrawl", "--max-credits", "100"]))
            .expect("valid invocation parses");
    assert_eq!(parsed.max_credits, 100);
}

/// Defaults: budget 250 (approved plan), concurrency 1.
#[test]
fn guard_defaults_are_budget_250_concurrency_1() {
    let parsed = competitor::parse_bench_live_args(args(&["--target", "firecrawl"]))
        .expect("valid invocation parses");
    assert_eq!(parsed.max_credits, 250);
    assert_eq!(parsed.concurrency, 1);
}

/// --concurrency requests parallelism (clamped later by the planner for
/// Firecrawl; parsing only validates the value).
#[test]
fn concurrency_flag_is_parsed() {
    let parsed =
        competitor::parse_bench_live_args(args(&["--target", "firecrawl", "--concurrency", "8"]))
            .expect("valid invocation parses");
    assert_eq!(parsed.concurrency, 8);
}

/// Non-numeric guard values are typed usage errors, not panics.
#[test]
fn non_numeric_max_credits_is_usage_error() {
    let err = competitor::parse_bench_live_args(args(&[
        "--target",
        "firecrawl",
        "--max-credits",
        "lots",
    ]))
    .expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
}

/// Zero and negative guard values are rejected at parse time.
#[test]
fn zero_or_negative_guard_values_are_rejected() {
    for bad in ["0", "-3"] {
        let err = competitor::parse_bench_live_args(args(&[
            "--target",
            "firecrawl",
            "--max-credits",
            bad,
        ]))
        .expect_err("must fail");
        assert!(matches!(err, BenchmarkError::Render(_)), "bad: {bad}");
    }
}

/// Repeated guard flags are rejected instead of silently last-wins.
#[test]
fn repeated_guard_flags_are_rejected() {
    let err = competitor::parse_bench_live_args(args(&[
        "--target",
        "firecrawl",
        "--max-credits",
        "10",
        "--max-credits",
        "20",
    ]))
    .expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
    let err = competitor::parse_bench_live_args(args(&[
        "--target",
        "firecrawl",
        "--concurrency",
        "1",
        "--concurrency",
        "2",
    ]))
    .expect_err("must fail");
    assert!(matches!(err, BenchmarkError::Render(_)));
}
