// `--max-chars`: the AI chunk-size guard.
//
// `--max-chars` renders in EVERY cargo configuration (ADR-0004): it replaced a
// token budget the cleaner can no longer compute, because the cleaner no longer
// tokenizes — it embeds through the domain `EmbeddingPort`. And since ADR-0004
// paso 6 removed the `ai` Cargo feature altogether, there is no configuration
// left where the AI group is absent: the flag must be answered by the ONE
// binary that exists. Run with:
//   cargo nextest run -p webfang_core --test max_chars_bound_test

//! `--max-chars` bounds (ADR-0004).
//!
//! `--max-chars` is the AI chunk-size guard: a chunk longer than the budget is
//! REJECTED by the semantic cleaner (`SemanticError::ChunkTooLarge`). Only the
//! lower bound is enforced — the retired `32_768` ceiling was Granite's Max
//! Sequence Length encoded as policy, and the effective ceiling against a
//! remote embedding endpoint is that provider's own context window, which it
//! reports per request as HTTP 400/413.
//!
//! The token-budget flag this replaced was retired together with the rest of
//! the ADR-0004 migration shims (issue #2119): it is no longer a flag, so a
//! stale invocation is answered by clap's standard "unexpected argument" and
//! NOT by a migration notice. The two cases at the bottom of this file pin
//! exactly that.
//!
//! The bound lives in the OptionsSpec SSOT (`NumericPolicy`), enforced at the
//! argv/env boundary by the same `numeric_binding` → `value_parser` path every
//! other migrated numeric flag already uses (`--max-pages`, `--timeout-secs`,
//! `--download-concurrency`, …). That path is a clap usage error, so the run
//! stops with `CliExit::UsageError` = **exit 64**, before any network I/O and
//! before the embedding provider is ever resolved — no `--clean-ai` is needed
//! here.
//!
//! Hermeticity (#1813 T1 lesson): every rejection case below still mounts a
//! wiremock. A pre-fix value that is NOT rejected lets the run proceed and dial
//! the real network; with the mock the observation is a fast, deterministic
//! exit 0 instead of a connectivity-dependent 69/74.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::{assert_snapshot_redacted, mount_single_page_site, BehavioralTest};
use std::time::Duration;

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

/// The opening of the migration notice the retired token-budget flag used to
/// print.
///
/// Counted (not just searched) so "does NOT warn" is a real assertion: a
/// surviving notice would read as a doubled message, and a missing one is
/// the silence the retirement exists to end. The literal is kept ONLY to
/// prove its own absence.
const RETIRED_MAX_TOKENS_NOTICE_MARKER: &str = "--max-tokens está obsoleto";

/// The opening of the migration notice the retired model flag used to print.
///
/// Same rationale as [`RETIRED_MAX_TOKENS_NOTICE_MARKER`]: kept only to prove
/// its own absence.
const RETIRED_AI_MODEL_NOTICE_MARKER: &str = "el modelo local ya no existe";

const RUN_TIMEOUT_SECS: u64 = 60;

/// Mount a single catch-all page so a run that PASSES validation has a
/// hermetic, instant success target instead of the real network — and so a run
/// that should have been rejected still resolves its mock if the rejection
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

/// The retired shims are GONE (issue #2119): `--max-tokens` and `--ai-model`
/// are no longer flags, so clap answers with its standard "unexpected
/// argument" usage error (exit 64) — NOT a migration notice.
///
/// The error text is clap's, verbatim: an operator who still has
/// `--max-tokens` in a script must be told the flag is unknown, because a
/// notice would imply the value was still honoured.
#[tokio::test]
async fn the_retired_shims_are_unknown_arguments() {
    for flag in ["--max-tokens", "--ai-model"] {
        let t = BehavioralTest::new().await;
        mount_single_page_site(&t).await;

        let output = scrape_with_flag(&t, flag, "4096").await;
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(EXIT_USAGE),
            "`{flag}` must be rejected as an unknown argument (exit 64); \
             stderr:\n{stderr}"
        );
        assert!(
            stderr.contains("unexpected argument"),
            "the rejection must be clap's standard unknown-argument error; \
             stderr:\n{stderr}"
        );
        assert!(
            !stderr.contains(RETIRED_MAX_TOKENS_NOTICE_MARKER),
            "a retired shim must not produce a migration notice; stderr:\n{stderr}"
        );
        assert!(
            !stderr.contains(RETIRED_AI_MODEL_NOTICE_MARKER),
            "a retired shim must not produce a migration notice; stderr:\n{stderr}"
        );
    }
}

/// The retired env doors are IGNORED, not rejected: an operator whose
/// systemd unit still exports `WEBFANG_MAX_TOKENS`, `WEBFANG_AI_MODEL_ID`,
/// `AI_MODEL_ID` or `WEBFANG_AI_ENGINE` gets a normal run, with no budget
/// converted and no notice printed.
///
/// An env var is not a flag, so clap cannot reject it — the only correct
/// behavior is to ignore it. What this pins is that ignoring it is SILENT:
/// the old notice text must not survive anywhere.
#[tokio::test]
async fn the_retired_shim_env_vars_are_ignored() {
    for var in [
        "WEBFANG_MAX_TOKENS",
        "WEBFANG_AI_MODEL_ID",
        "AI_MODEL_ID",
        "WEBFANG_AI_ENGINE",
    ] {
        let t = BehavioralTest::new().await;
        mount_single_page_site(&t).await;

        let output = t
            .scraper_cmd()
            .env(var, "4096")
            .arg("--single-page")
            .arg("--ignore-robots")
            .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
            .output()
            .expect("spawn webfang");
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert!(
            output.status.success(),
            "`{var}` is not a flag, so it must be ignored and the run must \
             succeed; stderr:\n{stderr}"
        );
        assert!(
            !stderr.contains(RETIRED_MAX_TOKENS_NOTICE_MARKER),
            "`{var}` must not produce a migration notice; stderr:\n{stderr}"
        );
        assert!(
            !stderr.contains(RETIRED_AI_MODEL_NOTICE_MARKER),
            "`{var}` must not produce the --ai-model migration notice; \
             stderr:\n{stderr}"
        );
    }
}

/// The spec declares the flag's behaviour (single source of truth), AND the
/// bound reaches the wire: `json_schema()` is the MCP-facing advertisement of
/// the same `NumericPolicy`.
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

/// The characters-per-token factor is ONE constant, read by every side that
/// needs it.
///
/// The retired `webfang_ai`'s `ModelConfig::default().chars_per_token` read
/// the same constant (it was pinned on the `webfang_ai` side by
/// `embedding_port_cleaner_test`, which COULD see both crates — this one
/// cannot, because `webfang_ai` depended on `webfang_core`). ADR-0004 paso 6
/// deleted the `webfang_ai` crate, and the token-budget shim that used the
/// factor for its conversion is gone too (issue #2119), so the domain's model
/// profiles are the readers that remain: the constant lives in
/// `options_spec::ai` and the pin below is the whole story.
#[test]
fn the_conversion_factor_is_a_single_shared_constant() {
    use webfang_core::domain::options_spec::ai::{DEFAULT_CHARS_PER_TOKEN, DEFAULT_MAX_CHARS};

    assert_eq!(
        DEFAULT_CHARS_PER_TOKEN, 3.0,
        "the documented factor must not drift silently"
    );
    // 32 768 tokens × 3.0 is exactly the shipped `--max-chars` default, which
    // is why that default is 98 304 and not a rounder-looking number.
    assert_eq!(
        DEFAULT_MAX_CHARS,
        DEFAULT_CHARS.parse::<usize>().expect("numeric"),
        "the retired token ceiling and the character default must describe \
         the same effective budget"
    );
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
