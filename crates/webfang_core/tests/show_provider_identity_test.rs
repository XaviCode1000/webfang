//! #2106 — `show` and `doctor` must name what the run REALLY uses.
//!
//! The `[[model_profiles]]` layer describes the LOCAL ONNX models that
//! ADR-0004 step 6 retired; no run consumes it. Before this fix, `show`
//! printed `granite-97m` as "the profile of the run" and `doctor` repeated
//! the same id, so an operator asking "what will this run use?" got a
//! retired model back.
//!
//! These tests run the REAL binary (the same surface the operator sees) and
//! assert on stdout: the effective embeddings provider is the run's identity,
//! and the model-profile numbers survive only as labelled historical metadata.
//!
//! The unit-level contract lives in `cli/commands/show.rs`; this file pins
//! the rendered output, which is the only thing an operator can observe.

#[path = "common/mod.rs"]
mod common;

use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

use common::cli_harness::cmd;

/// One embedding-capable provider: the identity a run would dial.
const KNOWN_PROVIDER_TOML: &str = "[[providers]]\nid = \"known\"\ndisplay_name = \"Known\"\n\
     kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text\"\n";

/// A second provider, so the explicit-id path can be told apart from the
/// config-order default.
const OTHER_PROVIDER_TOML: &str = "[[providers]]\nid = \"other\"\ndisplay_name = \"Other\"\n\
     kind = \"open_ai_compatible\"\nbase_url = \"https://other.example.test/v1\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text-v2\"\n";

/// Temp dir + `config.toml` seeded with `contents`. The dir is returned so it
/// outlives the test body.
fn seeded_config(contents: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, contents).expect("write config");
    (dir, config)
}

/// Run the `webfang` binary with `args` under `WEBFANG_CONFIG=config`.
fn run_with_config(args: &[&str], config: &Path) -> Output {
    cmd()
        .args(args)
        .env("WEBFANG_CONFIG", config)
        .output()
        .expect("run binary")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The part of the output that comes BEFORE the historical-metadata section.
///
/// The section header is the only place the word "histórica" appears, so the
/// split is exact. Everything before it is what the command claims the run
/// uses — the part that must never name a retired local model.
fn before_historical_section(stdout: &str) -> &str {
    stdout.split("histórica").next().unwrap_or(stdout)
}

// ============================================================================
// show
// ============================================================================

/// `show` names the effective embeddings provider: the identity of the run.
#[test]
fn show_names_the_effective_embeddings_provider() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["show"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("=== show: provider de embeddings efectivo ==="),
        "show must title the effective-provider section, got:\n{stdout}"
    );
    assert!(
        stdout.contains("id: known"),
        "show must name the resolved provider, got:\n{stdout}"
    );
    assert!(
        stdout.contains("base_url: https://api.example.test/v1"),
        "show must report what it read from the config, got:\n{stdout}"
    );
}

/// The defect itself: the retired local model is never presented as the
/// profile of the run.
#[test]
fn show_never_presents_a_retired_local_model_as_the_run_profile() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["show"], &config);
    let stdout = stdout_of(&output);

    let run_identity = before_historical_section(&stdout);
    assert!(
        !run_identity.contains("granite-97m"),
        "the run's identity must not name a retired local model, got:\n{run_identity}"
    );
    assert!(
        stdout.contains("metadata histórica"),
        "the profile numbers must survive as labelled historical metadata, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Ninguna corrida los consume"),
        "the historical section must say that no run consumes it, got:\n{stdout}"
    );
}

/// An id the operator named is resolved BY ID, and the output says where it
/// came from.
#[test]
fn show_resolves_the_explicitly_named_provider() {
    let (_dir, config) = seeded_config(&format!("{KNOWN_PROVIDER_TOML}{OTHER_PROVIDER_TOML}"));

    let output = run_with_config(&["--embedding-provider", "other", "show"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("id: other"),
        "show must resolve the named provider, got:\n{stdout}"
    );
    assert!(
        stdout.contains("origen: operatorio"),
        "show must say the provider came from the operator, got:\n{stdout}"
    );
}

/// An unknown id is a hard configuration error that names the id — and it
/// talks about a PROVIDER, never about a model profile: `--embedding-provider`
/// lives in the provider namespace (the namespace collision #2106 closes).
#[test]
fn show_with_an_unknown_provider_id_is_a_config_error_naming_the_id() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["--embedding-provider", "does-not-exist", "show"], &config);
    let stdout = stdout_of(&output);
    let stderr = stderr_of(&output);

    assert_eq!(
        output.status.code(),
        Some(78),
        "an unknown provider id is a configuration error (exit 78), got:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("does-not-exist"),
        "the error must name the unknown id, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("perfil de modelo"),
        "the error must not talk about a model profile — the id names a \
         provider, got:\n{stderr}"
    );
}

/// No configured provider: `show` says so. It does NOT fall back to a
/// shipped profile to fill the gap.
#[test]
fn show_without_any_provider_reports_no_embeddings_provider() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["show"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("sin provider de embeddings configurado"),
        "show must report the empty slot, got:\n{stdout}"
    );
    let run_identity = before_historical_section(&stdout);
    assert!(
        !run_identity.contains("granite-97m"),
        "an empty slot must not be filled with a retired local model, got:\n{run_identity}"
    );
}

// ============================================================================
// doctor
// ============================================================================

/// `doctor` keeps its diagnosis, and labels the profile numbers as history.
#[test]
fn doctor_labels_the_profile_numbers_as_historical_metadata() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["doctor"], &config);
    let stdout = stdout_of(&output);

    assert!(
        stdout.contains("id: known"),
        "doctor must name the resolved provider, got:\n{stdout}"
    );
    // Pre-existing contract (issue #1933): the Tramo D boundary stays
    // disclosed, because it is still true — no run consumes the profile.
    assert!(
        stdout.contains("Tramo D"),
        "doctor must keep naming the Tramo D boundary, got:\n{stdout}"
    );
    assert!(
        stdout.contains("no está cableado al run"),
        "doctor must keep saying the profile is not wired to the run, got:\n{stdout}"
    );
    let run_identity = before_historical_section(&stdout);
    assert!(
        !run_identity.contains("granite-97m"),
        "the run's identity must not name a retired local model, got:\n{run_identity}"
    );
}

/// Two diagnostics, one truth: `show` and `doctor` must resolve the same
/// provider (they read the same config).
#[test]
fn show_and_doctor_agree_on_the_effective_provider() {
    let (_dir, config) = seeded_config(&format!("{KNOWN_PROVIDER_TOML}{OTHER_PROVIDER_TOML}"));

    let show_stdout = stdout_of(&run_with_config(&["show"], &config));
    let doctor_stdout = stdout_of(&run_with_config(&["doctor"], &config));

    assert!(
        show_stdout.contains("id: known"),
        "show must name the config-order default, got:\n{show_stdout}"
    );
    assert!(
        doctor_stdout.contains("id: known"),
        "doctor must name the same provider, got:\n{doctor_stdout}"
    );
}
