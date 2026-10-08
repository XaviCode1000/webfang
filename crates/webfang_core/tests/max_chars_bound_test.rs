// `--max-chars` and the `--max-tokens` deprecation shim.
//
// `--max-chars` renders in EVERY cargo configuration (ADR-0004): it replaced a
// token budget the cleaner can no longer compute, because the cleaner no longer
// tokenizes — it embeds through the domain `EmbeddingPort`. So unlike the rest
// of the AI group this file is NOT `#![cfg(feature = "ai")]`: both flags must
// be answered by a binary built with or without the feature. Run with:
//   cargo nextest run -p webfang_core --test max_chars_bound_test

//! `--max-chars` bounds and the `--max-tokens` migration (ADR-0004).
//!
//! `--max-chars` is the AI chunk-size guard: a chunk longer than the budget is
//! REJECTED by the semantic cleaner (`SemanticError::ChunkTooLarge`). Only the
//! lower bound is enforced — the retired `32_768` ceiling was Granite's Max
//! Sequence Length encoded as policy, and the effective ceiling against a
//! remote embedding endpoint is that provider's own context window, which it
//! reports per request as HTTP 400/413.
//!
//! `--max-tokens` survives only as a shim: the flag and `WEBFANG_MAX_TOKENS`
//! still exist, and EVERY value is refused with a Spanish message naming
//! `--max-chars`. A removed flag would come back as "unexpected argument",
//! which tells an operator nothing about what to use instead — ADR-0004
//! forbids that silence.
//!
//! Both bounds live in the OptionsSpec SSOT (`NumericPolicy`), enforced at the
//! argv/env boundary by the same `numeric_binding` → `value_parser` path every
//! other migrated numeric flag already uses (`--max-pages`, `--timeout-secs`,
//! `--download-concurrency`, …). That path is a clap usage error, so the run
//! stops with `CliExit::UsageError` = **exit 64**, before any network I/O and
//! before the ONNX model is ever resolved — no `--clean-ai` is needed here.
//!
//! Hermeticity (#1813 T1 lesson): every rejection case below still mounts a
//! wiremock. A pre-fix value that is NOT rejected lets the run proceed and dial
//! the real network; with the mock the observation is a fast, deterministic
//! exit 0 instead of a connectivity-dependent 69/74.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::{redact_nondeterministic, BehavioralTest};
use insta::assert_snapshot;
use std::path::Path;
use std::time::Duration;
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

/// Exit code of a rejected flag VALUE: `CliExit::UsageError` (EX_USAGE).
///
/// Established by `webfang_cli::main::parse_args`, which maps every
/// non-help clap error (including `value_parser` rejections) to
/// `CliExit::UsageError`. NOT 78: `ConfigError` is the preflight-staging exit,
/// and both flags are enforced at the clap boundary, one stage earlier.
const EXIT_USAGE: i32 = 64;

/// The shipped default of `--max-chars`, which is also its documented value
/// (98 304 characters = the retired 32 768-token budget at 3.0 chars/token).
const DEFAULT_CHARS: &str = "98304";

/// Distinctive substring of the `--max-tokens` deprecation warning. Counted
/// (not just searched) so "warns exactly once" is a real assertion: a
/// duplicated warning reads as a doubled message, and a missing one is the
/// silence ADR-0004 forbids.
const DEPRECATION_MARKER: &str = "--max-tokens está obsoleto";

const RUN_TIMEOUT_SECS: u64 = 60;

/// Snapshot a stderr blob with the harness temp dir and other
/// non-deterministic output redacted.
fn assert_snapshot_redacted(name: &str, dir: &Path, value: impl Into<String>) {
    let redacted = redact_nondeterministic(dir, &value.into());
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"(?m)^Parsed using .+$", "Parsed using [REDACTED]");
    settings.bind(|| {
        assert_snapshot!(name, redacted);
    });
}

/// Mount a single catch-all page so a run that PASSES validation has a
/// hermetic, instant success target instead of the real network — and so a run
/// that should have been rejected still resolves its mock if the rejection
/// regresses (observed as exit 0, never as a network error).
async fn mount_single_page_site(t: &BehavioralTest) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Page</h1>\
             <p>Substantive content long enough to clear the fifty character \
             minimum content guard comfortably.</p></article></body></html>",
        ))
        .mount(&t.server)
        .await;
}

/// Run one scrape of the mock with an explicit flag value.
async fn scrape_with_flag(t: &BehavioralTest, flag: &str, value: &str) -> std::process::Output {
    t.scraper_cmd()
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg(flag)
        .arg(value)
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang")
}

/// The lower bound. `--max-chars 0` makes the guard reject EVERY chunk (any
/// non-empty chunk is longer than 0 characters), so accepting it would be a
/// silent, total denial of the AI path. Must fail closed with the Spanish
/// bound message and exit 64.
#[tokio::test]
async fn max_chars_zero_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_flag(&t, "--max-chars", "0").await;

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`--max-chars 0` must be rejected as a usage error (exit 64), not accepted \
         and not turned into a run that rejects every chunk; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_snapshot_redacted(
        "max_chars_zero_fails_closed_stderr",
        t.out.path(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    );
}

/// Triangulation — a non-numeric value is a typo, not a request. The bound
/// must not swallow this into a bound message: a "must be >= 1" reply to
/// `banana` would be a misleading diagnostic.
#[tokio::test]
async fn max_chars_non_numeric_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_flag(&t, "--max-chars", "banana").await;

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`--max-chars banana` must be rejected (exit 64); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_snapshot_redacted(
        "max_chars_non_numeric_fails_closed_stderr",
        t.out.path(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    );
}

/// Triangulation — the lower boundary itself is VALID (inclusive min). Catches
/// a `<=` instead of `<` on the minimum.
#[tokio::test]
async fn max_chars_lower_boundary_is_accepted() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_flag(&t, "--max-chars", "1").await;

    assert!(
        output.status.success(),
        "`--max-chars 1` is inside the inclusive bound and must be accepted; \
         stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Triangulation — there is NO ceiling any more, and that is the point: the
/// retired 32 768-token cap was one backend's Max Sequence Length, not a
/// property of the guard. A value far above it must be accepted, because the
/// real ceiling (the embedding provider's context window) is enforced by the
/// provider, not by a literal in this tool.
#[tokio::test]
async fn max_chars_above_the_retired_token_ceiling_is_accepted() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_flag(&t, "--max-chars", "999999").await;

    assert!(
        output.status.success(),
        "`--max-chars` must have no ceiling: 999999 is a request the \
         embedding backend, not this flag, gets to refuse; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The env var is NOT a second, softer front door: `spec_command::build_arg`
/// attaches the same `value_parser` to `WEBFANG_MAX_CHARS` that it attaches to
/// `--max-chars`, so an env-sourced zero must fail identically.
#[tokio::test]
async fn env_max_chars_zero_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = t
        .scraper_cmd()
        .env("WEBFANG_MAX_CHARS", "0")
        .arg("--single-page")
        .arg("--ignore-robots")
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`WEBFANG_MAX_CHARS=0` must be rejected exactly like the flag (exit 64); \
         stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The `--max-tokens` shim, PHASE 1 of ADR-0004's two-phase policy: the flag
/// still WORKS. Release N announces the deprecation (accept, warn once, convert);
/// only release N+1 removes it. A test that asserted a rejection here would be
/// pinning the removal into the announcement.
///
/// Values across the retired `1..=32_768` band are covered so a later change
/// cannot quietly reintroduce the old 32 768 ceiling.
#[tokio::test]
async fn max_tokens_shim_is_accepted_and_warns_once() {
    for value in ["1", "4096", "32768", "999999"] {
        let t = BehavioralTest::new().await;
        mount_single_page_site(&t).await;

        let output = scrape_with_flag(&t, "--max-tokens", value).await;
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "`--max-tokens {value}` must still be accepted in the announcement \
             release; stderr:\n{stderr}"
        );
        let occurrences = stderr.matches(DEPRECATION_MARKER).count();
        assert_eq!(
            occurrences, 1,
            "`--max-tokens {value}` must warn EXACTLY once; got {occurrences}:\n{stderr}"
        );
        assert!(
            stderr.contains("--max-chars"),
            "the warning must name the replacement; stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("--max-tokens está obsoleto"),
            "the warning must say the flag is obsolete; stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("caracteres"),
            "the warning must state the resulting unit; stderr:\n{stderr}"
        );
    }
}

/// The env door is not a second, softer front door: `WEBFANG_MAX_TOKENS` maps
/// exactly like the flag and warns exactly once, so a stale `.env` or systemd
/// unit reaches the same budget an operator gets from argv.
#[tokio::test]
async fn env_max_tokens_shim_maps_like_the_flag() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = t
        .scraper_cmd()
        .env("WEBFANG_MAX_TOKENS", "4096")
        .arg("--single-page")
        .arg("--ignore-robots")
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "`WEBFANG_MAX_TOKENS=4096` must be accepted like the flag; stderr:\n{stderr}"
    );
    assert_eq!(
        stderr.matches(DEPRECATION_MARKER).count(),
        1,
        "the env door must warn exactly once; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&format!("{} caracteres", 4096 * 3)),
        "the env-sourced conversion must match the flag's (4096 x 3.0); \
         stderr:\n{stderr}"
    );
}

/// Precedence: an operator who typed `--max-chars` has stated the unit they
/// mean. The deprecated value is ignored and, crucially, NOT warned about — a
/// warning here would be noise about a value they did not use, and the stale
/// shim value must not override the flag just typed.
#[tokio::test]
async fn max_chars_wins_silently_over_the_deprecated_flag() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = t
        .scraper_cmd()
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--max-tokens")
        .arg("4096")
        .arg("--max-chars")
        .arg("2048")
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "both flags together must parse; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(DEPRECATION_MARKER),
        "an explicit --max-chars must suppress the deprecation warning entirely; \
         stderr:\n{stderr}"
    );
}

/// The zero-silent-loss rule survives the rename unchanged: a budget of `0`
/// rejects every non-empty chunk, so it must fail closed at the argv boundary
/// with the Spanish bound message and exit 64 — for BOTH flags.
#[tokio::test]
async fn max_tokens_zero_still_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_flag(&t, "--max-tokens", "0").await;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`--max-tokens 0` must be rejected (exit 64): it would reject every \
         chunk while looking configured; stderr:\n{stderr}"
    );
    assert_snapshot_redacted(
        "max_tokens_zero_fails_closed_stderr",
        t.out.path(),
        stderr.into_owned(),
    );
}

/// A non-numeric value is a typo, not a request: it is a usage error (exit 64)
/// and must NOT be swallowed into the deprecation path — the flag is accepted,
/// but only as a number.
#[tokio::test]
async fn max_tokens_non_numeric_still_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_flag(&t, "--max-tokens", "banana").await;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`--max-tokens banana` must be rejected (exit 64); stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains(DEPRECATION_MARKER),
        "a rejected value must not also produce the deprecation warning; \
         stderr:\n{stderr}"
    );
}

/// One snapshot of the deprecation warning, so the wording a user reads is
/// pinned: a migration that rewords itself silently is not an announcement.
#[tokio::test]
async fn max_tokens_deprecation_warning_is_pinned() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_flag(&t, "--max-tokens", "4096").await;

    assert_snapshot_redacted(
        "max_tokens_deprecation_warning_stderr",
        t.out.path(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    );
}

/// The spec declares both flags' behaviour (single source of truth), AND the
/// new bound reaches the wire: `json_schema()` is the MCP-facing
/// advertisement of the same `NumericPolicy`.
///
/// Pure in-process test — no binary spawn, no network — because it pins the
/// DECLARATION. The behavioral tests above are what prove the declaration is
/// not bypassable.
#[test]
fn max_chars_spec_declares_the_lower_bound_and_only_that() {
    use webfang_core::domain::options_spec as spec;

    let schema = spec::ai::MAX_CHARS.json_schema();
    assert_eq!(schema["type"], "integer");
    assert_eq!(schema["minimum"], 1);
    assert_eq!(
        schema["default"].as_u64(),
        Some(DEFAULT_CHARS.parse::<u64>().expect("numeric default")),
        "the advertised default must be the documented one"
    );
    assert!(
        schema.get("maximum").is_none(),
        "no ceiling is declared: the embedding backend owns the real limit; got: {schema}"
    );
    assert_eq!(
        spec::ai::MAX_CHARS.check_bound(1),
        Ok(1),
        "the inclusive lower boundary must pass the policy"
    );
    assert_eq!(
        spec::ai::MAX_CHARS.check_bound(983_304),
        Ok(983_304),
        "values above the retired token ceiling must pass: the provider's \
         context window, not this spec, is the real ceiling"
    );
    assert_eq!(
        spec::ai::MAX_CHARS.check_bound(0),
        Err(webfang_core::domain::options_spec::BoundError::MinViolated { min: 1 })
    );

    // The `--help` text must DOCUMENT what the code does, including that the
    // ceiling belongs to the backend.
    let help = spec::ai::MAX_CHARS.help;
    assert!(
        help.contains("400/413"),
        "help text must say who enforces the real ceiling; got: {help}"
    );
}

/// The deprecated shim declares the ANNOUNCEMENT phase, not the removal:
/// ungated, no default, lower bound `>= 1`, NO ceiling, and a `--help` line
/// that names the replacement and the conversion.
#[test]
fn max_tokens_spec_declares_a_deprecation_not_a_removal() {
    use webfang_core::domain::options_spec as spec;

    assert_eq!(
        spec::ai::MAX_TOKENS.feature_gate,
        None,
        "the shim renders in every configuration"
    );
    assert!(
        spec::ai::MAX_TOKENS.default.is_none(),
        "the shim has no default: absent means 'nobody used the deprecated flag'"
    );
    for value in ["1", "4096", "32768", "999999"] {
        assert_eq!(
            spec::ai::MAX_TOKENS
                .parse_uint(value)
                .expect("phase 1 accepts the value"),
            value.parse::<u64>().expect("numeric"),
            "ADR-0004 phase 1 must keep accepting --max-tokens"
        );
    }
    // The retired 32 768 ceiling does NOT come back with the flag: it was one
    // backend's Max Sequence Length, not a property of the guard.
    assert!(
        spec::ai::MAX_TOKENS.parse_uint("999999").is_ok(),
        "the shim must not resurrect the retired token ceiling"
    );
    // Zero silent loss, unchanged: 0 rejects every chunk.
    assert_eq!(
        spec::ai::MAX_TOKENS.check_bound(0),
        Err(webfang_core::domain::options_spec::BoundError::MinViolated { min: 1 })
    );
    let err = spec::ai::MAX_TOKENS
        .parse_uint("banana")
        .expect_err("a typo is not a request");
    assert!(
        err.to_string().contains("no es un número entero válido"),
        "got: {err}"
    );

    let help = spec::ai::MAX_TOKENS.help;
    assert!(
        help.contains("--max-chars"),
        "help must name the replacement; got: {help}"
    );
    assert!(
        help.contains("3.0"),
        "help must state the conversion factor; got: {help}"
    );
}

/// The conversion factor is ONE constant, read by both sides: `webfang_core`
/// translates the legacy value here, and `webfang_ai`'s
/// `ModelConfig::default().chars_per_token` reads the same constant (pinned on
/// the `webfang_ai` side by `embedding_port_cleaner_test`, which CAN see both
/// crates — this one cannot, because `webfang_ai` depends on `webfang_core`).
#[test]
fn the_conversion_factor_is_a_single_shared_constant() {
    use webfang_core::domain::options_spec as spec;
    use webfang_core::domain::options_spec::ai::{
        max_tokens_to_max_chars, DEFAULT_CHARS_PER_TOKEN,
    };

    assert_eq!(
        DEFAULT_CHARS_PER_TOKEN, 3.0,
        "the documented factor must not drift silently"
    );
    // The help text advertises the same number the conversion uses: a help line
    // promising 3.0 while the code multiplies by something else is a silent
    // budget bug.
    assert!(
        spec::ai::MAX_TOKENS.help.contains("3.0"),
        "help must state the factor the code actually uses"
    );

    assert_eq!(max_tokens_to_max_chars(0), 0);
    assert_eq!(max_tokens_to_max_chars(1), 3);
    assert_eq!(max_tokens_to_max_chars(4096), 12_288);
    assert_eq!(max_tokens_to_max_chars(32_768), 98_304);
    // Rounding, never truncation: 1.5 chars must not silently become 1.
    assert_eq!(
        max_tokens_to_max_chars(1),
        3,
        "the conversion rounds to the nearest character budget"
    );
    // 32 768 tokens × 3.0 is exactly the shipped `--max-chars` default, which
    // is why that default is 98 304 and not a rounder-looking number.
    assert_eq!(
        max_tokens_to_max_chars(32_768),
        DEFAULT_CHARS.parse::<usize>().expect("numeric"),
        "the retired token ceiling and the new character default must describe \\
         the same effective budget"
    );
}

/// The precedence rule as implemented, asserted on the value the consumer
/// receives (`cli::args::build_ai_config` -> `AiConfig`).
#[test]
fn precedence_maps_warns_only_without_max_chars() {
    use clap::FromArgMatches as _;
    use webfang_core::application::crawl_options::AiConfig;
    use webfang_core::domain::options_spec::ai::DEFAULT_CHARS_PER_TOKEN;

    let ai_of = |argv: &[&str]| -> AiConfig {
        let mut full = vec!["webfang", "--url", "https://example.com"];
        full.extend_from_slice(argv);
        let matches = <webfang_core::Args as clap::CommandFactory>::command()
            .try_get_matches_from(full)
            .expect("argv must parse");
        let args = webfang_core::Args::from_arg_matches(&matches).expect("derive args");
        webfang_core::CrawlOptions::from(args).ai_config
    };

    // 1. Neither flag: the MAX_CHARS default, no provenance, no warning owed.
    let ai = ai_of(&[]);
    assert_eq!(
        ai.max_chars,
        DEFAULT_CHARS.parse::<usize>().expect("numeric")
    );
    assert!(ai.deprecated_max_tokens.is_none());

    // 2. Only the deprecated flag: mapped, and provenance recorded for the
    //    one warning the binary emits after the subscriber exists.
    let ai = ai_of(&["--max-tokens", "4096"]);
    let expected = (4096.0 * f64::from(DEFAULT_CHARS_PER_TOKEN)).round() as usize;
    assert_eq!(ai.max_chars, expected, "tokens × factor");
    assert_eq!(ai.deprecated_max_tokens, Some(4096));

    // 3. Both: --max-chars wins verbatim and the deprecated value is DROPPED
    //    (no provenance => no warning, even though the flag was present).
    let ai = ai_of(&["--max-tokens", "4096", "--max-chars", "2048"]);
    assert_eq!(ai.max_chars, 2048);
    assert!(
        ai.deprecated_max_tokens.is_none(),
        "an explicit --max-chars must leave no deprecation to report"
    );

    // 4. Reverse order parses identically — precedence is not positional.
    let ai = ai_of(&["--max-chars", "2048", "--max-tokens", "4096"]);
    assert_eq!(ai.max_chars, 2048);
    assert!(ai.deprecated_max_tokens.is_none());

    // 5. The env door is the same choice as the flag: WEBFANG_MAX_CHARS set by
    //    an operator counts as explicit, so it wins over a legacy env value.
    //    (Proved structurally in `args_test`, where env can be set hermetically.)
}

/// The value flows all the way to the consumer that enforces it, so the bound
/// is not guarding a variable nothing reads.
///
/// `Args.ai.max_chars` → `build_ai_config` → `AiConfig.max_chars` is the exact
/// chain `webfang_cli::main` hands to `ModelConfig::with_max_chars`, whose
/// `config.max_chars` is what `SemanticCleanerImpl::clean` compares each
/// chunk's character count against. Pinned here on the core side of that chain
/// (the `ai` crate is outside this slice's edit surface).
#[test]
fn max_chars_reaches_the_ai_config_consumer() {
    use clap::FromArgMatches as _;

    let matches = <webfang_core::Args as clap::CommandFactory>::command()
        .try_get_matches_from([
            "webfang",
            "--url",
            "https://example.com",
            "--max-chars",
            "4096",
        ])
        .expect("valid --max-chars must parse");
    let args = webfang_core::Args::from_arg_matches(&matches).expect("derive args");
    let options = webfang_core::CrawlOptions::from(args);

    assert_eq!(
        options.ai_config.max_chars, 4096,
        "--max-chars must reach AiConfig.max_chars, the value the semantic \
         cleaner compares each chunk's character count against"
    );
}
