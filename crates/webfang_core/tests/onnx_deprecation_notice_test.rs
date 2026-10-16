// ADR-0004 step 5: the deprecation announcements that reach the terminal.
//
// This is the "release de deprecación" half of ADR-0004 (`Decision` step 5 /
// `Compatibilidad`): it ANNOUNCES the announced break, it does not remove
// anything. Two notices exist, both emitted ONCE per run:
//
// 1. `ONNX_DEPRECATION_NOTICE` — what disappears (the `ai` Cargo feature and
//    ONNX local), what survives un-gated (`--clean-ai`, `--offline`), which
//    shims keep working for at least 2 more minor releases, where to migrate
//    (`webfang connect` / `doctor` / `show`), and where to object.
// 2. The orphan-threshold warning — the shipped `0.3` was calibrated for
//    Granite, and with the only calibrated model gone a remote endpoint
//    retains differently under the same number.
//
// Both are gated on the SAME predicate: the run actually TOUCHES the AI
// surface. That is deliberate and load-bearing, not an optimization — see
// `cli::args::ai::record_adr0004_notices`. A run that never asked for AI has
// nothing to migrate and nothing to recalibrate, so it stays silent; a notice
// on every invocation would train operators to ignore stderr, which is worse
// than no notice at all.
//
// HOW they reach the terminal is the other half of the design and is pinned by
// this file existing at all: argument resolution runs BEFORE
// `init_logging_dual`, so a `tracing::warn!` raised during argv normalization
// is silently dropped (#796). The verdict is therefore computed at parse time
// (`PendingNotices`, in `cli::args::ai`) and rendered by `webfang_cli::main`
// right after the subscriber is installed, as its own `warn!` per notice — the
// `--max-tokens` shape, NOT the `preflight_notes` buffer. That buffer's replay
// prints the note as both the event message and a structured field: invisible
// for a 40-character substitution notice, an unreadable ~1200-character line
// for a deprecation. Every test here COUNTS its marker, which is what exposed
// that duplication in the first place.
//
// Hermeticity (#1813 T1 lesson): every case mounts a wiremock, so a run whose
// preflight regressed fails fast against a local server instead of dialing the
// real network.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::{redact_nondeterministic, BehavioralTest};
use insta::assert_snapshot;
use std::path::Path;
use std::time::Duration;
use webfang_core::cli::args::ai::{ONNX_DEPRECATION_NOTICE_MARKER, ORPHAN_THRESHOLD_NOTICE_MARKER};
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

const RUN_TIMEOUT_SECS: u64 = 60;

/// An explicit `--threshold` that is NOT the default, so the orphan warning's
/// own precondition ("the user did not pass one") is genuinely exercised
/// rather than satisfied by an identical value.
const EXPLICIT_THRESHOLD: &str = "0.55";

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

/// Snapshot name suffix for a run that went through `--clean-ai`.
///
/// `--clean-ai` is FEATURE-GATED: without the `ai` feature the flag is a
/// configuration error (exit 78, "recompilá con --features ai"), and the run
/// stops before scraping. With the feature on, the same invocation resolves an
/// endpoint and the scrape proceeds to completion. The two WARN lines this file
/// exists to pin are byte-identical either way; only the trailing result block
/// differs.
///
/// An insta snapshot is feature-BLIND — it cannot express "this under `ai`,
/// that without it". One committed file could therefore only ever describe one
/// of the two builds, and `test-full` (`cargo nextest run --all-features`) and
/// `test-core` (`cargo nextest run --test '*'`) would disagree on which. So the
/// NAME carries the discriminator, and both configurations get pinned.
///
/// The feature set is an exact proxy for the binary's behavior, not a guess:
/// `common::cli_harness::webfang_path()` derives the child `cargo build`'s
/// `--features` from this very crate's `cfg!(feature = "ai")`, so the binary
/// under test always carries the feature the test was compiled with. See
/// `cli_harness.rs`, the `active_features` block.
fn clean_ai_snapshot_name(base: &str) -> String {
    let suffix = if cfg!(feature = "ai") {
        "ai_feature_on"
    } else {
        "ai_feature_off"
    };
    format!("{base}_{suffix}")
}

/// Mount a single catch-all page so a run that reaches the scrape path has a
/// hermetic, instant target instead of the real network.
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

/// How many times `marker` appears in the captured stderr.
///
/// A PLAIN substring count, which is the strongest form available: each notice
/// reaches stderr as one `tracing::warn!` whose Spanish prose appears once, as
/// the event message, with the facts travelling separately in English
/// structured fields. So a count of 1 is one notice, full stop.
///
/// This is deliberately NOT a line count. The first cut of this feature routed
/// the notices through `preflight_notes`, whose replay is
/// `warn!(preflight_note = %note, "{note}")` — the compact formatter then
/// printed the same ~600 characters twice per line (message + field value). A
/// line count would have hidden that; a substring count is what exposed it.
fn notice_events(stderr: &str, marker: &str) -> usize {
    stderr.matches(marker).count()
}

/// One scrape of the mock with the given extra arguments.
async fn scrape(t: &BehavioralTest, extra: &[&str]) -> std::process::Output {
    let mut cmd = t.scraper_cmd();
    cmd.arg("--single-page").arg("--ignore-robots");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang")
}

/// The main announcement: a run that asks for AI semantic cleaning hears what
/// is going away, what survives, which shims are still supported, where to
/// migrate and where to object — EXACTLY once.
///
/// `--clean-ai` on a build without the `ai` feature still reaches the notice:
/// the replay happens right after `init_logging_dual`, which is upstream of the
/// `--clean-ai` capability gate. That ordering is the point — an operator on a
/// non-AI binary is exactly the operator who most needs to know the flag is
/// going away.
#[tokio::test]
async fn onnx_deprecation_notice_fires_once_when_clean_ai_is_used() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape(&t, &["--clean-ai"]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        notice_events(&stderr, ONNX_DEPRECATION_NOTICE_MARKER),
        1,
        "the ONNX deprecation notice must appear EXACTLY once on an AI-surface \
         run; stderr:\n{stderr}"
    );
    // The notice has to be actionable, not merely present: it must name what
    // disappears, what survives, the shims, the migration commands and the
    // objection channel (ADR-0004 `Compatibilidad`, all five in order).
    for required in [
        "cargo build --features ai",
        "--clean-ai",
        "--embedding-provider",
        "--offline",
        "--ai-model",
        "--max-tokens",
        "WEBFANG_MAX_TOKENS",
        "AI_MODEL_ID",
        "WEBFANG_AI_MODEL_ID",
        "WEBFANG_AI_ENGINE",
        "webfang connect",
        "webfang doctor",
        "webfang show",
        "https://github.com/XaviCode1000/webfang/issues/1997",
    ] {
        assert!(
            stderr.contains(required),
            "the notice must name `{required}`; stderr:\n{stderr}"
        );
    }
    assert_snapshot_redacted(
        &clean_ai_snapshot_name("onnx_deprecation_notice_clean_ai_stderr"),
        t.out.path(),
        stderr.into_owned(),
    );
}

/// Negative control against vacuous matching: a run that never touched the AI
/// surface must stay silent. Without this, the positive test above would still
/// pass if the notice fired unconditionally.
#[tokio::test]
async fn onnx_deprecation_notice_does_not_fire_when_the_ai_surface_is_untouched() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape(&t, &[]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the plain run must succeed; stderr:\n{stderr}"
    );
    assert_eq!(
        notice_events(&stderr, ONNX_DEPRECATION_NOTICE_MARKER),
        0,
        "a run that never asked for AI must not be told about the AI migration; \
         stderr:\n{stderr}"
    );
    assert_eq!(
        notice_events(&stderr, ORPHAN_THRESHOLD_NOTICE_MARKER),
        0,
        "the orphan-threshold warning rides the same gate; stderr:\n{stderr}"
    );
}

/// The env legs of the trigger. `AI_MODEL_ID` and `WEBFANG_AI_ENGINE` are read
/// below clap (`webfang_ai`'s `compat::read_ai_model_id_with` and
/// `EngineConfig::from_env`), so from argv they are invisible and must be probed
/// directly — an operator who only exported one of them is still an AI-surface
/// run.
#[tokio::test]
async fn ai_engine_env_alone_reaches_the_deprecation_notice() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = t
        .scraper_cmd()
        .env("WEBFANG_AI_ENGINE", "single")
        .arg("--single-page")
        .arg("--ignore-robots")
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        notice_events(&stderr, ONNX_DEPRECATION_NOTICE_MARKER),
        1,
        "`WEBFANG_AI_ENGINE` alone must reach the notice; stderr:\n{stderr}"
    );
}

/// The orphaned-default mitigation, exactly once, while the threshold is still
/// the shipped default.
///
/// The `0.3` was calibrated against Granite. ONNX local is the only model it
/// was ever calibrated on, so while the threshold is the default and no model
/// is calibrated, the number means something different than it says.
#[tokio::test]
async fn orphan_threshold_warning_fires_once_with_the_default_threshold() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape(&t, &["--clean-ai"]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        notice_events(&stderr, ORPHAN_THRESHOLD_NOTICE_MARKER),
        1,
        "the orphan-threshold warning must appear EXACTLY once while the \
         threshold is the default; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("--threshold"),
        "the warning must name the knob the operator should reach for; \
         stderr:\n{stderr}"
    );
    assert_snapshot_redacted(
        &clean_ai_snapshot_name("orphan_threshold_warning_default_stderr"),
        t.out.path(),
        stderr.into_owned(),
    );
}

/// An operator who CHOSE a threshold has taken the calibration into their own
/// hands; the orphan warning would be noise about a decision they already made.
#[tokio::test]
async fn orphan_threshold_warning_does_not_fire_with_an_explicit_threshold() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = scrape(&t, &["--clean-ai", "--threshold", EXPLICIT_THRESHOLD]).await;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        notice_events(&stderr, ORPHAN_THRESHOLD_NOTICE_MARKER),
        0,
        "an explicit --threshold must suppress the orphan warning entirely; \
         stderr:\n{stderr}"
    );
    // The main notice is unaffected: the AI surface was still touched.
    assert_eq!(
        notice_events(&stderr, ONNX_DEPRECATION_NOTICE_MARKER),
        1,
        "choosing a threshold does not cancel the ONNX deprecation notice; \
         stderr:\n{stderr}"
    );
}

/// The warning must quote the DEFAULT it is complaining about, so it has to be
/// the same number the spec advertises and the same one the runtime default
/// carries. A warning that says "0.3" while the code defaults to something else
/// is a lie about the very value it exists to qualify.
#[test]
fn the_announced_default_threshold_is_the_number_the_runtime_actually_defaults_to() {
    use webfang_core::application::crawl_options::AiConfig;
    use webfang_core::domain::options_spec as spec;

    assert_eq!(spec::ai::DEFAULT_THRESHOLD, 0.3);
    assert_eq!(
        spec::ai::THRESHOLD
            .default
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("0.3"),
        "the spec's rendered default_value and the numeric SSOT must agree"
    );
    assert_eq!(
        AiConfig::default().threshold,
        spec::ai::DEFAULT_THRESHOLD,
        "the value the pipeline runs with must be the value the warning names"
    );
}
