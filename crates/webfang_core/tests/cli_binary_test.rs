//! CLI binary integration tests
//!
//! Tests the actual `webfang` binary using `assert_cmd`.
//! These tests verify the binary behaves correctly for edge cases
//! without requiring network access.
//!
//! Run with: cargo nextest run --test-threads 2 cli_binary_test

#[path = "common/cli_harness.rs"]
mod common;
use common::{cmd, redact_nondeterministic};

use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Seeded `[[providers]]` block shared by the doctor arrange blocks: one
/// known embedding provider so the budget render has something to name.
const KNOWN_PROVIDER_TOML: &str = "[[providers]]\nid = \"known\"\ndisplay_name = \"Known\"\n\
     kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
     auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
     model = \"nomic-embed-text\"\n";

/// Temp dir + `config.toml` seeded with `contents` for show/doctor arrange
/// blocks (issue #1933). The dir is returned so it outlives the test body.
fn seeded_config(contents: &str) -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("config.toml");
    std::fs::write(&config, contents).expect("write config");
    (dir, config)
}

/// Run the `webfang` binary with `args` under `WEBFANG_CONFIG=config`.
fn run_with_config(args: &[&str], config: &Path) -> std::process::Output {
    cmd()
        .args(args)
        .env("WEBFANG_CONFIG", config)
        .output()
        .expect("run binary")
}

/// Split a spawned binary's output into owned stdout/stderr strings for the
/// assertions below.
fn output_text(output: &std::process::Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Shared assert for the R4-1 malformed-config contract: exit 78 with stderr
/// naming the file's own TOML problem. The `doctor` test additionally asserts
/// the absent false diagnosis inline.
fn assert_malformed_config_refused(output: &std::process::Output, stdout: &str, stderr: &str) {
    assert_eq!(
        output.status.code(),
        Some(78),
        "a malformed config is a configuration error (exit 78), got:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("no es TOML válido"),
        "the error must name the file's own problem, got:\n{stderr}"
    );
    assert!(
        stderr.contains("config.toml"),
        "the error must name the offending file, got:\n{stderr}"
    );
}

// ============================================================================
// Tests: Binary error handling
// ============================================================================

/// Test that running without --url shows an error message
#[test]
fn test_no_url_shows_error() {
    let output = cmd().output().expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_no_url_shows_error",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
}

/// Test that an invalid URL shows an error
#[test]
fn test_invalid_url_shows_error() {
    // CLI validates URL and returns error message
    let output = cmd()
        .arg("--url")
        .arg("not-a-url")
        .output()
        .expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_invalid_url_shows_error",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
}

// ============================================================================
// Tests: Binary help and version
// ============================================================================

/// Test that --help contains scraper description
/// Test that --help prints usage and exits with code 0.
/// Only runs when the `adaptive-selectors` feature is enabled, because the
/// snapshot includes the `--adaptive-selectors` flag (present only with
/// `--features adaptive-selectors`).
///
/// The AI flags (`--clean-ai`, `--threshold`, `--max-chars`, `--max-tokens`,
/// `--ai-model`, `--offline`) render in EVERY configuration since ADR-0004
/// removed the `ai` Cargo feature, so they no longer discriminate the
/// snapshot between two builds.
#[cfg(feature = "adaptive-selectors")]
#[test]
fn test_help_contains_scraper() {
    let output = cmd().arg("--help").output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"(?m)^Parsed using .+$", "Parsed using [REDACTED]");
    settings.bind(|| {
        insta::assert_snapshot!(
            "test_help_contains_scraper",
            redact_nondeterministic(Path::new("__no_temp__"), &stdout)
        );
    });
}

/// Test that --version outputs the workspace version and exits with code 0.
///
/// Deliberately NOT an insta snapshot: the version string changes by design on
/// every release-plz bump, so a golden file here fails every Release PR that
/// does not also remember to rewrite it (webfang#1543). The expected value is
/// derived from the same `CARGO_PKG_VERSION` the binary embeds — assert the
/// contract, not a frozen copy of it.
#[test]
fn test_version() {
    let output = cmd().arg("--version").output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.trim(),
        format!("webfang {}", env!("CARGO_PKG_VERSION")),
        "`webfang --version` must print the current package version"
    );
}

// ============================================================================
// Tests: Dry-run mode
// ============================================================================

/// Test that --dry-run with a valid URL does not fail (but may fail on network)
#[test]
#[ignore = "requires network access"]
fn test_dry_run_with_url() {
    cmd()
        .arg("--url")
        .arg("https://example.com")
        .arg("--dry-run")
        .assert()
        .success();
}

// ============================================================================
// Tests: doctor / show help text (false-provenance guard)
// ============================================================================

/// `webfang --help` must NOT promise that `show` dumps what a run will use:
/// ADR-0004 wires `[[model_profiles]]` into the cleaner in Tramo D, so the
/// help text describes a configured budget, not a consumed one. The snapshot
/// in `test_help_contains_scraper` baked in the old, false wording; this
/// asserts the corrected contract directly against the binary.
///
/// Issue #1933 (ADR-0004 step 4).
#[test]
fn test_issue_1933_show_about_does_not_claim_the_run_uses_the_profile() {
    let output = cmd().args(["show", "--help"]).output().expect("run binary");
    assert!(output.status.success(), "expected success");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stdout.contains("the run will use"),
        "show's help must not claim the run uses the configured profile \
         (Tramo D is not wired), got:\n{stdout}"
    );
    assert!(
        stdout.contains("effective embeddings provider"),
        "show's help must name the identity the run really uses — the \
         resolved embeddings provider (issue #2106), got:\n{stdout}"
    );
    assert!(
        stdout.contains("historical model-profile metadata"),
        "show's help must label the profile numbers as history — no run \
         consumes them (issue #2106), got:\n{stdout}"
    );
}

/// `webfang doctor` must disclose the Tramo D boundary in its budget block:
/// the profile numbers are CONFIGURED, not consumed by a run yet. Captured
/// from the real binary so the assertion is on what the operator actually
/// sees, not on an in-source string mirrored by the test.
///
/// Issue #1933 (ADR-0004 step 4).
#[test]
fn test_issue_1933_doctor_budget_discloses_the_tramo_d_boundary() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    let output = run_with_config(&["doctor"], &config);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // `doctor` may exit non-zero if the reachability probe fails (no network
    // in CI); the budget block prints regardless. Assert on the text, not the
    // exit code.
    assert!(
        stdout.contains("Tramo D"),
        "doctor's budget block must name the Tramo D boundary, got:\n{stdout}"
    );
    assert!(
        stdout.contains("no está cableado al run"),
        "doctor must say the profile is not wired to the run yet, got:\n{stdout}"
    );
}

/// A config that still declares the removed `local_onnx` kind must fail to
/// load, naming the value.
///
/// This replaces the contract that made the problem invisible: until 3.0.0 the
/// binary exited 0 here, because `ProviderKind::LocalOnnx` mapped to
/// `Ok(None)` and `doctor` read that as "the local pool serves". ADR-0004
/// paso 6 removed the pool, so the same input must now be a hard failure —
/// never a run without embeddings, never a green diagnostic.
#[test]
fn issue_2099_config_with_the_removed_local_onnx_kind_is_rejected() {
    let (_dir, config) = seeded_config(
        "[[providers]]\nid = \"local\"\ndisplay_name = \"Local\"\n\
         kind = \"local_onnx\"\nbase_url = \"http://127.0.0.1:8080/\"\n\
         auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n",
    );

    let output = run_with_config(&["--embedding-provider", "local", "doctor"], &config);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_ne!(
        output.status.code(),
        Some(0),
        "a removed provider kind must not load, got stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("local_onnx"),
        "the error must name the rejected value, got:\n{stderr}"
    );
}

// ============================================================================
// Tests: show / doctor rendered output (issue #1933, ADR-0004 step 4)
//
// The unit tests in `cli::commands::show` match `CliExit` variants only. The
// RENDERED contract — provenance labels, the citation, the budget origins and
// the Tramo D boundary — is pinned here against the real binary, because that
// text is what the operator actually reads.
// ============================================================================

/// No provider configured: the run identity is EMPTY (never a shipped model
/// profile), and the repo-verified profiles survive only as labelled
/// historical metadata — with their repo citation, the native dim and the
/// profile context (issue #2106).
#[test]
fn show_default_render_names_the_repo_verified_profiles_as_history() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["show"], &config);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "show must exit 0 with no provider configured, got:\n{stdout}"
    );
    assert!(
        stdout.contains("sin provider de embeddings configurado"),
        "the run identity must be the EMPTY provider slot, got:\n{stdout}"
    );
    assert!(
        stdout.contains("perfil: granite-97m (repo-verified)"),
        "the historical profile must carry its repo-verified provenance, got:\n{stdout}"
    );
    assert!(
        stdout.contains("citation: docs/adr/0004-onnx-local-removal.md"),
        "the profile must cite its repo source, got:\n{stdout}"
    );
    assert!(
        stdout.contains("dim (nativa): 384"),
        "granite-97m is 384d native, got:\n{stdout}"
    );
    assert!(
        stdout.contains("context: 32768 tokens"),
        "the profile context must be shown, got:\n{stdout}"
    );
    assert!(
        stdout.contains("Ninguna corrida los consume"),
        "the profile numbers must be labelled as history no run consumes, got:\n{stdout}"
    );
}

/// Operator override of `dim`: the output drops to `operator-override`, shows
/// the OVERRIDDEN value, and warns that the audited identity was overwritten.
#[test]
fn show_operator_override_renders_override_provenance_value_and_warning() {
    let (_dir, config) =
        seeded_config("[[model_profiles.profiles]]\nid = \"granite-97m\"\ndim = 512\n");

    let output = run_with_config(&["show"], &config);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "show must exit 0 with an operator override, got:\n{stdout}"
    );
    assert!(
        stdout.contains("perfil: granite-97m (operator-override)"),
        "an overridden profile is operator-override, got:\n{stdout}"
    );
    assert!(
        stdout.contains("dim (nativa): 512"),
        "the overridden dim must be the rendered value, got:\n{stdout}"
    );
    assert!(
        stdout.contains("ATENCIÓN") && stdout.contains("identidad auditada"),
        "the override must warn that the audited identity was overwritten, got:\n{stdout}"
    );
}

/// An explicit `--max-chars` must appear in the budget block with its
/// operator origin named.
#[test]
fn show_budget_names_the_operator_origin_for_an_explicit_max_chars() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["--max-chars", "4096", "show"], &config);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "show must exit 0 with an explicit --max-chars, got:\n{stdout}"
    );
    assert!(
        stdout.contains("max_chars: 4096"),
        "the explicit --max-chars value must be shown, got:\n{stdout}"
    );
    assert!(
        stdout.contains("operatorio (--max-chars o WEBFANG_MAX_CHARS)"),
        "an explicit --max-chars must be labelled operator-supplied, got:\n{stdout}"
    );
}

/// Without flags the budget block names the spec default for BOTH knobs —
/// `show` may not dress a default up as a choice the operator made.
#[test]
fn show_budget_names_the_spec_default_without_flags() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["show"], &config);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(
        stdout.matches("default del spec").count(),
        2,
        "max_chars AND threshold must both name the spec default when no flag \
         is given, got:\n{stdout}"
    );
}

/// An explicit `--threshold` must appear in the budget block with its operator
/// origin — the flag is global, so it precedes the `show` subcommand.
#[test]
fn show_budget_names_the_operator_origin_for_an_explicit_threshold() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["--threshold", "0.5", "show"], &config);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "show must exit 0 with an explicit --threshold, got:\n{stdout}"
    );
    assert!(
        stdout.contains("threshold: 0.5"),
        "the explicit --threshold value must be shown, got:\n{stdout}"
    );
    assert!(
        stdout.contains("operatorio (--threshold)"),
        "an explicit --threshold must be labelled operator-supplied, got:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("default del spec").count(),
        1,
        "max_chars is still the spec default in this run, got:\n{stdout}"
    );
}

/// The Tramo D boundary note must appear in `show` output: the numbers are
/// shown, but consuming them in the cleaner is explicitly not wired yet.
#[test]
fn show_output_discloses_the_tramo_d_boundary() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["show"], &config);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("Tramo D"),
        "show must name the Tramo D boundary, got:\n{stdout}"
    );
    assert!(
        stdout.contains("no está cableado al run"),
        "show must say the profile is not wired to the run yet, got:\n{stdout}"
    );
}

/// An unknown PROVIDER id is a configuration error that NAMES the id, in
/// Spanish — never a silent fallback to the config-order default.
///
/// `--embedding-provider` names a provider in both subcommands (issue
/// #2106): the error talks about the embedding capability, never about a
/// model profile.
#[test]
fn show_unknown_provider_id_is_a_spanish_error_naming_the_id() {
    let (_dir, config) = seeded_config("");

    let output = run_with_config(&["--embedding-provider", "no-such-model", "show"], &config);
    let (stdout, stderr) = output_text(&output);

    assert_eq!(
        output.status.code(),
        Some(78),
        "an unknown provider id is a configuration error (exit 78), got:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("no-such-model"),
        "the error must name the unknown provider id, got:\n{stderr}"
    );
    assert!(
        stderr.contains("embedding"),
        "the error must name the embedding capability, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("perfil de modelo"),
        "the error must not talk about a model profile — the id names a \
         provider, got:\n{stderr}"
    );
}

/// `doctor`'s budget block names the threshold origin on its own render path:
/// `operatorio` with an explicit `--threshold`, `default del spec` without it
/// (the provenance contract `show` asserts, pinned for doctor as well).
#[test]
fn test_doctor_budget_names_the_threshold_origin() {
    let (_dir, config) = seeded_config(KNOWN_PROVIDER_TOML);

    // No flag: the threshold is the spec default, and doctor must say so.
    // (doctor may exit non-zero if the reachability probe fails — no network
    // in CI; the budget block prints regardless. Assert on the text.)
    let default = run_with_config(&["doctor"], &config);
    let default_stdout = String::from_utf8_lossy(&default.stdout);
    assert!(
        default_stdout.contains("umbral de relevancia: 0.3 (default del spec)"),
        "doctor must label the threshold a spec default when no flag is given, \
         got:\n{default_stdout}"
    );

    let explicit = run_with_config(&["--threshold", "0.5", "doctor"], &config);
    let explicit_stdout = String::from_utf8_lossy(&explicit.stdout);
    assert!(
        explicit_stdout.contains("umbral de relevancia: 0.5 (operatorio)"),
        "doctor must label an explicit --threshold operator-supplied, \
         got:\n{explicit_stdout}"
    );
}

// ============================================================================
// Tests: doctor / show config diagnostics (R4-1)
//
// A malformed config used to degrade silently to defaults inside the
// subcommand (ConfigDefaults::load), and the tracing::error! that carried
// the real cause was dropped — subcommands dispatch before the tracing
// subscriber exists. The operator saw "No hay ningún provider en el archivo
// de configuración": a false diagnosis about a file full of providers.
// The contract now: the error names the file's own problem, exit 78, and
// the false-providers message never appears.
// ============================================================================

/// `doctor` with a malformed `WEBFANG_CONFIG`: exit 78, stderr names the
/// file's TOML problem, and the false "No hay ningún provider" diagnosis is
/// absent from every stream.
#[test]
fn doctor_names_a_malformed_config_instead_of_inventing_a_diagnosis() {
    let (_dir, config) = seeded_config("format = ");

    let output = run_with_config(&["doctor"], &config);
    let (stdout, stderr) = output_text(&output);

    assert_malformed_config_refused(&output, &stdout, &stderr);
    assert!(
        !stdout.contains("No hay ningún provider") && !stderr.contains("No hay ningún provider"),
        "the false no-providers diagnosis must not appear, got:\n{stdout}\n{stderr}"
    );
}

/// `show` with a malformed `WEBFANG_CONFIG`: the same contract as `doctor` —
/// exit 78 and stderr naming the file's problem, never a degraded render of
/// repo defaults dressed up as the operator's configuration.
#[test]
fn show_names_a_malformed_config_instead_of_inventing_a_diagnosis() {
    let (_dir, config) = seeded_config("format = ");

    let output = run_with_config(&["show"], &config);
    let (stdout, stderr) = output_text(&output);

    assert_malformed_config_refused(&output, &stdout, &stderr);
}

/// `doctor` with a NONEXISTENT `WEBFANG_CONFIG` override is fail-closed,
/// exactly like the run: `load_explicit` classifies a missing override as
/// `NotFound` — an error, never a degradation (pinned by
/// `load_explicit_rejects_a_missing_file` in `cli::config`). The diagnostic
/// must therefore exit 78 naming the missing path, not pretend the operator
/// has no providers.
#[test]
fn doctor_refuses_a_missing_explicit_config_override() {
    let dir = TempDir::new().expect("temp dir");
    let config = dir.path().join("no-such-config.toml");

    let output = run_with_config(&["doctor"], &config);
    let (stdout, stderr) = output_text(&output);

    assert_eq!(
        output.status.code(),
        Some(78),
        "a missing explicit override is a configuration error (exit 78), got:\n{stdout}\n{stderr}"
    );
    assert!(
        stderr.contains("no existe el archivo de configuración"),
        "the error must say the file does not exist, got:\n{stderr}"
    );
    assert!(
        stderr.contains("no-such-config.toml"),
        "the error must name the missing path, got:\n{stderr}"
    );
}

// ============================================================================
// Tests: Feature flags
// ============================================================================

/// Test that --quiet flag is accepted
#[test]
fn test_quiet_flag_accepted() {
    // Should not fail at argument parsing (will fail at network without URL)
    let output = cmd().arg("--quiet").output().expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_quiet_flag_accepted",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
}

/// Test that --dry-run flag is accepted
#[test]
fn test_dry_run_flag_accepted() {
    let output = cmd().arg("--dry-run").output().expect("run binary");
    assert!(!output.status.success(), "expected failure");
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(
        "test_dry_run_flag_accepted",
        redact_nondeterministic(Path::new("__no_temp__"), &stderr)
    );
}

// ============================================================================
// Tests: Single-page scraping
// ============================================================================

#[tokio::test]
async fn test_single_page_requests_only_seed_and_writes_output() {
    let mock_server = MockServer::start().await;
    let output_dir = TempDir::new().expect("create temp output dir");
    let seed_html = r#"
        <html>
            <head><title>Single Page Test</title></head>
            <body>
                <main>
                    <article>
                        <h1>Single Page Test</h1>
                        <p>This page has enough meaningful content for the fallback extractor to produce a usable document.</p>
                        <p>The linked page must not be requested while --single-page is active, because discovery is skipped.</p>
                        <a href="/linked">Linked page that should not be fetched</a>
                    </article>
                </main>
            </body>
        </html>
    "#;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(seed_html))
        .expect(1)
        .named("single-page seed request")
        .mount(&mock_server)
        .await;

    cmd()
        .arg("--url")
        .arg(mock_server.uri())
        .arg("--single-page")
        .arg("--output")
        .arg(output_dir.path())
        .arg("--quiet")
        .assert()
        .success();

    let received_requests = mock_server
        .received_requests()
        .await
        .expect("request recording should be enabled");
    // Filter out infrastructure requests (robots.txt) — we only care about
    // page fetches. The robots_utils fix now correctly fetches robots.txt
    // from the mock server origin (previously it hardcoded https:// and failed).
    let page_requests: Vec<_> = received_requests
        .iter()
        .filter(|r| r.url.path() != "/robots.txt")
        .collect();
    assert_eq!(page_requests.len(), 1);
    assert_eq!(page_requests[0].url.path(), "/");

    let output_entries = std::fs::read_dir(output_dir.path())
        .expect("read output dir")
        .count();
    assert!(output_entries > 0, "single-page scrape should write output");
}

#[tokio::test]
async fn test_single_page_custom_timeout_is_used_by_scrape_client() {
    let mock_server = MockServer::start().await;
    let output_dir = TempDir::new().expect("create temp output dir");

    // Per-request timeout is the configured ceiling for EACH attempt.
    // FIX-1 F-08 (#1231): timeouts are retriable, so the fetch runs 1
    // initial attempt + `max_retries` (default 3) retries — each attempt
    // still capped by the custom 1s timeout — then exits 69 with the
    // last timeout. The 3 retry WARN lines in the stderr snapshot are
    // the observability of that budget. (Mid-body transients also retry:
    // #649.)
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("slow response content")
                .set_delay(Duration::from_secs(2)),
        )
        .expect(4) // 1 attempt + 3 retries (default max_retries)
        .named("single-page timeout request")
        .mount(&mock_server)
        .await;

    let output = cmd()
        .arg("--url")
        .arg(format!("{}/slow", mock_server.uri()))
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("1")
        .arg("--output")
        .arg(output_dir.path())
        .arg("--quiet")
        .output()
        .expect("run binary");
    assert_eq!(output.status.code(), Some(69), "expected exit code 69");
    let stderr = String::from_utf8_lossy(&output.stderr);
    // The custom timeout value is this test's whole premise, and the exact
    // value is the one thing `redact_nondeterministic` can no longer show:
    // #1631 collapses the network-failure surface to a single `<NET_ERR>` token,
    // because an unreachable host refuses on Unix but times out on Windows and
    // one snapshot cannot hold both. What survives without this line: exit 69
    // plus the 4-request `expect` above prove a timeout under the mock's 2s
    // delay fired and was retried 3 times. What it cannot prove is WHICH value
    // — `timeout_secs` is a `u64`, so 0 and 1 both time out at 0s and 1s and
    // both yield exit 69. Only the message separates them, so pin it here on
    // the un-redacted stderr. Stable across platforms by construction:
    // `request timed out after {0}s` is webfang's own `DownloadError::Timeout`
    // variant (`domain/downloader_port.rs:99`), not a std or OS string.
    assert!(
        stderr.contains("request timed out after 1s"),
        "the --timeout-secs 1 value must reach the scrape client; stderr was: {stderr}"
    );
    insta::assert_snapshot!(
        "test_single_page_custom_timeout_is_used_by_scrape_client",
        redact_nondeterministic(output_dir.path(), &stderr)
    );
}
