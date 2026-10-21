// The orphaned-default relevance threshold (ADR-0004 step 5, spec T5).
//
// The shipped `0.3` relevance threshold was calibrated for Granite, and
// Granite is the local ONNX model ADR-0004 removes: no model left in the box
// is calibrated for it, so a remote endpoint retains differently under the
// same number. The CLI therefore warns while the threshold is still the
// default.
//
// This file was born when the ONNX deprecation announcement and the six
// migration shims were retired together (issue #2119): that announcement
// shared this file's harness, and the announcement's own contracts are gone
// with it. What SURVIVES is the threshold qualification — it is a PERMANENT
// notice (issue #2167), not a deprecation with an end date, because no
// model in the box is calibrated for the shipped default and that stays true
// until the default itself moves.
//
// HOW the warning reaches the terminal is the other half of the design and is
// pinned by this file existing at all: argument resolution runs BEFORE
// `init_logging_dual`, so a `tracing::warn!` raised during argv normalization
// is silently dropped (#796). The verdict is therefore computed at parse time
// (`PendingNotices`, in `cli::args::ai`) and rendered by `webfang_cli::main`
// right after the subscriber is installed, as its own `warn!` — the
// `preflight_notes` buffer's replay prints the note as both the event message
// and a structured field, and a line repeated like that is unreadable for a
// notice this long.
//
// Hermeticity (#1813 T1 lesson): every case mounts a wiremock, so a run whose
// preflight regressed fails fast against a local server instead of dialing the
// real network.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::{assert_snapshot_redacted, mount_single_page_site, BehavioralTest};
use std::time::Duration;
use webfang_core::cli::args::ai::ORPHAN_THRESHOLD_NOTICE_MARKER;

const RUN_TIMEOUT_SECS: u64 = 60;

/// An explicit `--threshold` that is NOT the default, so the orphan warning's
/// own precondition ("the user did not pass one") is genuinely exercised
/// rather than satisfied by an identical value.
const EXPLICIT_THRESHOLD: &str = "0.55";

/// Mount a single catch-all page so a run that reaches the scrape path has a
/// hermetic target instead of the real network.
async fn ai_surface_stderr(extra: &[&str]) -> (BehavioralTest, String) {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;
    let output = scrape_ai_surface(&t, extra).await;
    (t, String::from_utf8_lossy(&output.stderr).into_owned())
}

/// How many times `marker` appears in the captured stderr.
///
/// A PLAIN substring count, which is the strongest form available: the notice
/// reaches stderr as one `tracing::warn!` whose Spanish prose appears once, as
/// the event message, with the facts travelling separately in English
/// structured fields. So a count of 1 is one notice, full stop.
fn notice_events(stderr: &str, marker: &str) -> usize {
    stderr.matches(marker).count()
}

/// One AI-surface scrape, hermetic in EVERY configuration.
///
/// `--clean-ai` no longer reaches a local model loader: since ADR-0004 paso 6
/// the semantic cleaner runs over a remote `EmbeddingPort`, and a run with no
/// resolvable provider stops at the configuration error (exit 78) BEFORE any
/// request leaves the process. `--offline` is kept on the invocation because
/// it is one of the two flags the announcement promised survives un-gated, and
/// its semantics (loopback allowed) are unchanged.
///
/// The run therefore ends in exit 78 with the honest "no embedding provider"
/// message — and that is EXACTLY what these tests want to observe: the notice
/// is rendered by `webfang_cli::main` right after `init_logging_dual`, which
/// is UPSTREAM of the preflight gates. An operator on a binary that cannot
/// honor `--clean-ai` is precisely the operator who most needs to know the
/// threshold is not calibrated for their endpoint.
///
/// No `HF_HOME` juggling any more: the hf_hub cache is gone with the ONNX
/// stack, and the hermetic `WEBFANG_CONFIG` the harness installs (an empty
/// file) is what guarantees no provider is resolved.
async fn scrape_ai_surface(t: &BehavioralTest, extra: &[&str]) -> std::process::Output {
    let mut cmd = t.scraper_cmd();
    cmd.arg("--single-page")
        .arg("--ignore-robots")
        .arg("--clean-ai")
        .arg("--offline");
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang")
}

/// The orphaned-default mitigation, exactly once, while the threshold is still
/// the shipped default.
///
/// The `0.3` was calibrated against Granite. ONNX local is the only model it
/// was ever calibrated on, so while the threshold is the default and no model
/// is calibrated, the number means something different than it says.
#[tokio::test]
async fn orphan_threshold_warning_fires_once_with_the_default_threshold() {
    let (t, stderr) = ai_surface_stderr(&[]).await;

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
        "orphan_threshold_warning_default_stderr",
        t.out.path(),
        stderr,
    );
}

/// An operator who CHOSE a threshold has taken the calibration into their own
/// hands; the orphan warning would be noise about a decision they already made.
#[tokio::test]
async fn orphan_threshold_warning_does_not_fire_with_an_explicit_threshold() {
    let (_t, stderr) = ai_surface_stderr(&["--threshold", EXPLICIT_THRESHOLD]).await;

    assert_eq!(
        notice_events(&stderr, ORPHAN_THRESHOLD_NOTICE_MARKER),
        0,
        "an explicit --threshold must suppress the orphan warning entirely; \
         stderr:\n{stderr}"
    );
}

/// Negative control against vacuous matching: a run that never touched the AI
/// surface must stay silent. Without this, the positive test above would
/// still pass if the notice fired unconditionally.
#[tokio::test]
async fn orphan_threshold_warning_does_not_fire_without_the_ai_surface() {
    let t = BehavioralTest::new().await;
    mount_single_page_site(&t).await;

    let output = t
        .scraper_cmd()
        .arg("--single-page")
        .arg("--ignore-robots")
        .timeout(Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "the plain run must succeed; stderr:\n{stderr}"
    );
    assert_eq!(
        notice_events(&stderr, ORPHAN_THRESHOLD_NOTICE_MARKER),
        0,
        "a run that never asked for AI must not be told about the threshold \
         calibration; stderr:\n{stderr}"
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
