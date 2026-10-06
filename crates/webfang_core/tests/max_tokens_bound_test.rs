// `--max-tokens` only exists when the `ai` cargo feature is on
// (`spec_command::ai_args` emits the AI group only under `cfg(feature = "ai")`).
// Without the gate this whole crate compiles to zero tests — the silent-skip
// defect T5 exists to close — so the gate is stated, not assumed. Run with:
//   cargo nextest run -p webfang_core --features ai --test max_tokens_bound_test
#![cfg(feature = "ai")]

//! `--max-tokens` bounds (issue #1813, slice T2).
//!
//! `--max-tokens` is the AI chunk-size guard: a chunk whose tokenization
//! exceeds it is REJECTED by the semantic cleaner
//! (`SemanticError::ChunkTooLarge`). Before this slice the spec declared
//! `ValueKind::uint_unbounded()` and clap bound the built-in `usize` parser,
//! so `--max-tokens 0` was accepted — a guard that rejects every chunk by
//! construction — and any ceiling above the tokenizer's own truncation limit
//! was accepted too, even though no input could ever reach it.
//!
//! The bound lives in the OptionsSpec SSOT (`NumericPolicy`), enforced at the
//! argv/env boundary by the same `numeric_binding` → `value_parser` path every
//! other migrated numeric flag already uses (`--max-pages`, `--timeout-secs`,
//! `--download-concurrency`, …). That path is a clap usage error, so the run
//! stops with `CliExit::UsageError` = **exit 64**, before any network I/O and
//! before the ONNX model is ever resolved — no `--clean-ai` is needed here.
//!
//! Hermeticity (#1813 T1 lesson): every rejection case below still mounts a
//! wiremock. Pre-fix these values do NOT fail, so the run proceeds and would
//! otherwise dial the real network; with the mock the pre-fix observation is a
//! fast, deterministic exit 0 instead of a connectivity-dependent 69/74.
//!
//! The ceiling is `32_768` and equals the default on purpose: it is the
//! documented Max Sequence Length of the default embedding model
//! (`granite-embedding-97m-multilingual-r2`, same for the 311m-r2 fallback) AND
//! `MiniLmTokenizer::DEFAULT_MAX_LENGTH`, which truncates with
//! `.min(self.max_length)`. A higher cap could never fire, so the operator may
//! only LOWER the guard.

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
/// and `--max-tokens` is enforced at the clap boundary, one stage earlier.
const EXIT_USAGE: i32 = 64;

/// Inclusive ceiling, equal to the model/tokenizer max sequence length.
const CEILING: &str = "32768";

const RUN_TIMEOUT_SECS: u64 = 60;

/// Snapshot a stderr blob with the harness temp dir and other
/// non-deterministic output redacted.
///
/// Lives at the crate root on purpose: insta derives a snapshot's on-disk
/// location from the module path where `assert_snapshot!` expands.
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

/// Run one scrape of the mock with an explicit `--max-tokens`.
async fn scrape_with_max_tokens(t: &BehavioralTest, value: &str) -> std::process::Output {
    t.scraper_cmd()
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--max-tokens")
        .arg(value)
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang")
}

/// #1813 slice 2 — the lower bound. `--max-tokens 0` makes the guard reject
/// EVERY chunk (`seq_len() > 0` is true for any non-empty chunk), so it was a
/// silent, total denial of service on the AI path. Must fail closed with the
/// Spanish bound message and exit 64.
#[tokio::test]
async fn max_tokens_zero_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_max_tokens(&t, "0").await;

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`--max-tokens 0` must be rejected as a usage error (exit 64), not accepted \
         and not turned into a run that rejects every chunk; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_snapshot_redacted(
        "max_tokens_zero_fails_closed_stderr",
        t.out.path(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    );
}

/// #1813 slice 2 — the upper bound, at its first violating value. `32769` is
/// one token past the model's Max Sequence Length, so it can never be honoured:
/// the tokenizer truncates to 32768 first. Accepting it would advertise a
/// capability that does not exist. Boundary-adjacent on purpose (not 999999) so
/// an off-by-one in the comparison is caught rather than hidden.
#[tokio::test]
async fn max_tokens_above_ceiling_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_max_tokens(&t, "32769").await;

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`--max-tokens 32769` must be rejected (exit 64); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_snapshot_redacted(
        "max_tokens_above_ceiling_fails_closed_stderr",
        t.out.path(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    );
}

/// #1813 triangulation — a ceiling exactly at the bound is VALID. Without this,
/// a `<` instead of `<=` in the comparison would pass both rejection tests
/// above while rejecting the shipped default.
#[tokio::test]
async fn max_tokens_at_ceiling_is_accepted() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_max_tokens(&t, CEILING).await;

    assert!(
        output.status.success(),
        "`--max-tokens {CEILING}` is the documented default and must be accepted; \
         stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// #1813 triangulation — the lower boundary itself is VALID (inclusive min).
/// Catches a `<=` instead of `<` on the minimum.
#[tokio::test]
async fn max_tokens_lower_boundary_is_accepted() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_max_tokens(&t, "1").await;

    assert!(
        output.status.success(),
        "`--max-tokens 1` is inside the inclusive bound and must be accepted; \
         stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// #1813 triangulation — a non-numeric value is a typo, not a request. The
/// bound must not swallow this into a bound message: a "must be <= 32768" reply
/// to `banana` would be a misleading diagnostic.
#[tokio::test]
async fn max_tokens_non_numeric_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape_with_max_tokens(&t, "banana").await;

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`--max-tokens banana` must be rejected (exit 64); stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_snapshot_redacted(
        "max_tokens_non_numeric_fails_closed_stderr",
        t.out.path(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    );
}

/// #1813 triangulation — the env var is NOT a second, softer front door.
/// `spec_command::build_arg` attaches the same `value_parser` to
/// `WEBFANG_MAX_TOKENS` that it attaches to `--max-tokens`, so an env-sourced
/// zero must fail identically. (Mirrors `env_rate_limit_burst_non_numeric_fails_closed`
/// from slice T1, which proved the same for `--rate-limit-burst`.)
#[tokio::test]
async fn env_max_tokens_zero_fails_closed() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = t
        .scraper_cmd()
        .env("WEBFANG_MAX_TOKENS", "0")
        .arg("--single-page")
        .arg("--ignore-robots")
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");

    assert_eq!(
        output.status.code(),
        Some(EXIT_USAGE),
        "`WEBFANG_MAX_TOKENS=0` must be rejected exactly like the flag (exit 64); \
         stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The spec declares the bound (single source of truth), AND it reaches the
/// wire: `json_schema()` is the MCP-facing advertisement of the same
/// `NumericPolicy`, so a bound enforced on the CLI but absent from the schema
/// would advertise an impossible value to an MCP client.
///
/// Pure in-process test — no binary spawn, no network — because it pins the
/// DECLARATION. `max_tokens_zero_fails_closed` above is what proves the
/// declaration is not bypassable.
#[test]
fn max_tokens_spec_declares_both_bounds() {
    use webfang_core::domain::options_spec as spec;

    let schema = spec::ai::MAX_TOKENS.json_schema();
    assert_eq!(schema["type"], "integer");
    assert_eq!(schema["minimum"], 1);
    assert_eq!(schema["maximum"], 32768);

    assert_eq!(
        spec::ai::MAX_TOKENS.check_bound(1),
        Ok(1),
        "the inclusive lower boundary must pass the policy"
    );
    assert_eq!(
        spec::ai::MAX_TOKENS.check_bound(32_768),
        Ok(32_768),
        "the ceiling must pass the policy (inclusive, and equal to the default)"
    );
    assert_eq!(
        spec::ai::MAX_TOKENS.check_bound(0),
        Err(webfang_core::domain::options_spec::BoundError::MinViolated { min: 1 })
    );
    assert_eq!(
        spec::ai::MAX_TOKENS.check_bound(32_769),
        Err(webfang_core::domain::options_spec::BoundError::MaxViolated { max: 32_768 })
    );

    // The `--help` text must DOCUMENT the ceiling the code enforces: an
    // operator who cannot see the bound cannot know why 40000 was refused.
    assert!(
        spec::ai::MAX_TOKENS.help.contains("32768"),
        "help text must name the ceiling; got: {}",
        spec::ai::MAX_TOKENS.help
    );
}

/// The value flows all the way to the consumer that enforces it, so the bound
/// is not guarding a variable nothing reads.
///
/// `Args.ai.max_tokens` → `build_ai_config` → `AiConfig.max_tokens` is the exact
/// chain `webfang_cli::main` hands to `ModelConfig::with_max_tokens`, whose
/// `config.max_tokens` is what `SemanticCleanerImpl::clean` compares against
/// `input.seq_len()`. Pinned here on the core side of that chain (the `ai`
/// crate is outside this slice's edit surface).
#[test]
fn max_tokens_reaches_the_ai_config_consumer() {
    use clap::FromArgMatches as _;

    let matches = <webfang_core::Args as clap::CommandFactory>::command()
        .try_get_matches_from([
            "webfang",
            "--url",
            "https://example.com",
            "--max-tokens",
            "4096",
        ])
        .expect("valid --max-tokens must parse");
    let args = webfang_core::Args::from_arg_matches(&matches).expect("derive args");
    let options = webfang_core::CrawlOptions::from(args);

    assert_eq!(
        options.ai_config.max_tokens, 4096,
        "--max-tokens must reach AiConfig.max_tokens, the value the semantic \
         cleaner compares each chunk's seq_len() against"
    );
}
