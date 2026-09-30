//! Behavioral tests: the config loader must never lie about what it read (#1659).
//!
//! Before #1659 `ConfigDefaults::load` degraded to defaults on EVERY IO error,
//! so a `config.toml` that was unreadable, was a directory, or was not UTF-8
//! produced a run indistinguishable from one where the operator's config had
//! been applied. Since #1651 `docs/src/cli-reference.md` publishes that
//! fallback as the `WEBFANG_CONFIG` contract, so the dishonesty was public.
//!
//! These tests drive the real binary through the shared harness, because the
//! defect is only observable at the CLI boundary: the exit code and the stderr
//! text an operator actually sees.
//!
//! Run with:
//! `cargo nextest run -p webfang_core --test config_default_contract_test`
//!
//! Scoping notes, stated honestly:
//! - That a VALID override is *applied* (not merely accepted) is pinned by
//!   `budget_override_test`, which already proves TOML values reach the
//!   normalized run. These tests pin the contract around acceptance and
//!   rejection, which is what changed.
//! - `permission_denied` is `#[cfg(unix)]`: the mode bits are the mechanism.
//!   A privileged runner bypasses them, so that test asserts nothing rather
//!   than asserting something the OS will not enforce.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "common/cli_harness.rs"]
mod common;

use std::path::Path;

use common::{cmd, redact_nondeterministic, BehavioralTest};
use insta::assert_snapshot;
use webfang_core::cli::error::EXIT_CONFIG;
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

/// A valid seed URL.
///
/// The failing cases never touch the network — the config gate is step 5 of
/// startup, long before any fetch — but a URL is still required to get past
/// `resolve_url`, which exits 64 without one.
const SEED_URL: &str = "https://example.com/";

/// Assert a snapshot of stderr, redacting the temp dir, ANSI codes, dynamic
/// ports and the trailing OS error detail.
///
/// The `(os error N)` tail is normalized because the same `Io` variant renders
/// `Is a directory (os error 21)` on Linux and a different sentence with a
/// different number elsewhere. The snapshot must pin OUR message — that it
/// names the path and says what went wrong — not the platform's wording.
fn assert_stderr_snapshot(name: &str, dir: &Path, stderr: &str) {
    let redacted = redact_nondeterministic(dir, stderr);
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"\(os error \d+\)", "(os error N)");
    settings.bind(|| {
        assert_snapshot!(name, redacted);
    });
}

/// Assert a snapshot of stderr that embeds no temp path, and so needs no
/// directory redaction.
///
/// Deliberately defined HERE rather than reusing the harness's
/// `assert_snapshot_plain`: insta derives a snapshot's on-disk folder from the
/// module where `assert_snapshot!` expands, so calling a wrapper that lives in
/// `common` writes the snapshot to `tests/common/snapshots/` — the exact
/// misplacement the shared harness documents. This file is a test-target root,
/// so the snapshot lands in `tests/snapshots/` beside every other one.
///
/// Passing a dummy dir to the redactor is NOT an option: `redact_temp_path`
/// substitutes the directory string wherever it appears, so `Path::new(".")`
/// would rewrite the `.` inside `config.toml` into `<OUT_DIR>` and bake that
/// artifact into the committed snapshot.
fn assert_plain_stderr_snapshot(name: &str, stderr: &str) {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"\x1b\[[0-9;]*m", "");
    settings.bind(|| {
        assert_snapshot!(name, stderr);
    });
}

/// Mount the single discovery request a `--dry-run` makes, so the success
/// cases terminate hermetically without a real fetch.
async fn mount_discovery(t: &BehavioralTest) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>t</body></html>"))
        .mount(&t.server)
        .await;
}

/// Run the binary against `config_path` as an explicit override and assert the
/// run stopped at the config gate with `EXIT_CONFIG`.
///
/// Returns the captured stderr so the caller can pin the message.
fn run_expecting_config_error(config_path: &Path) -> String {
    let out = cmd()
        .arg("--url")
        .arg(SEED_URL)
        .env("WEBFANG_CONFIG", config_path)
        .output()
        .expect("spawn webfang");

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(i32::from(EXIT_CONFIG)),
        "an unusable explicit override must exit {EXIT_CONFIG} and name the path.\nstderr:\n{stderr}"
    );
    stderr
}

// ---------------------------------------------------------------------------
// The benign case: a missing PLATFORM DEFAULT still degrades silently
// ---------------------------------------------------------------------------

/// The one degradation that must stay silent: no override, no config file at
/// the platform path. This is the "fresh install" state, and it must NOT start
/// failing — only an explicit override may.
#[tokio::test]
async fn missing_platform_default_config_degrades_silently() {
    let t = BehavioralTest::new().await;
    mount_discovery(&t).await;

    // A config base with nothing in it: <base>/webfang/config.toml is absent.
    let empty_base = tempfile::TempDir::new().expect("create empty config base");

    let out = t
        .scraper_cmd()
        .arg("--dry-run")
        // Drop the harness override so the PLATFORM path is what gets read.
        .env_remove("WEBFANG_CONFIG")
        .env("XDG_CONFIG_HOME", empty_base.path())
        .output()
        .expect("spawn webfang");

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "a missing platform default must not fail the run.\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("archivo de configuración"),
        "the benign missing-file case must stay silent, got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// The regression: every OTHER IO error used to degrade silently too
// ---------------------------------------------------------------------------

/// The exact reproduction from the issue: the path exists, it is a directory,
/// `read_to_string` fails with `EISDIR`, and before #1659 the run exited 0
/// having silently used defaults.
#[test]
fn explicit_override_pointing_at_a_directory_exits_78() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let config_path = tmp.path().join("config.toml");
    std::fs::create_dir(&config_path).expect("create directory named config.toml");

    let stderr = run_expecting_config_error(&config_path);
    assert_stderr_snapshot("override_is_a_directory", tmp.path(), &stderr);
}

/// A `config.toml` that is not valid UTF-8 never reaches the TOML parser —
/// `read_to_string` rejects the bytes first — so it must be reported as an
/// unreadable file, not as a parse failure.
#[test]
fn explicit_override_with_invalid_utf8_exits_78() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, [0xff_u8, 0xfe, 0x00]).expect("write invalid UTF-8");

    let stderr = run_expecting_config_error(&config_path);
    assert_stderr_snapshot("override_is_invalid_utf8", tmp.path(), &stderr);
}

/// A malformed TOML file used to be the ONE case that was loud (`error!`) while
/// still degrading. An explicit override now refuses it outright, because the
/// operator named this file.
#[test]
fn explicit_override_with_malformed_toml_exits_78() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, "this is [[[ not valid toml").expect("write malformed TOML");

    let stderr = run_expecting_config_error(&config_path);
    assert_stderr_snapshot("override_is_malformed_toml", tmp.path(), &stderr);
}

/// A permission-denied config is the failure a developer actually hits. Unix
/// only because the mode bits are the mechanism; a privileged runner bypasses
/// them, so the test returns early instead of asserting an unenforceable fact.
#[cfg(unix)]
#[test]
fn explicit_override_that_cannot_be_read_exits_78() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, "format = \"json\"\n").expect("write config");
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o000))
        .expect("chmod 000");

    if std::fs::read_to_string(&config_path).is_ok() {
        // Privileges that bypass the mode bits: untestable here, not broken.
        return;
    }

    let stderr = run_expecting_config_error(&config_path);
    assert_stderr_snapshot("override_is_permission_denied", tmp.path(), &stderr);
}

// ---------------------------------------------------------------------------
// The typo case: an explicit override that does not resolve must FAIL
// ---------------------------------------------------------------------------

/// The issue's headline requirement: with an explicit override set, a path that
/// does not exist must fail instead of degrading to defaults. `docs/src/
/// cli-reference.md` used to promise the opposite, which is why that sentence
/// had to change too.
#[test]
fn nonexistent_explicit_override_exits_78() {
    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let config_path = tmp.path().join("typo.toml");

    let stderr = run_expecting_config_error(&config_path);
    assert_stderr_snapshot("override_does_not_exist", tmp.path(), &stderr);
}

/// A relative override would resolve against whatever working directory the
/// process inherited — arbitrary under a daemon or service manager — so it is
/// rejected rather than guessed at.
#[test]
fn relative_explicit_override_exits_78() {
    let out = cmd()
        .arg("--url")
        .arg(SEED_URL)
        .env("WEBFANG_CONFIG", "webfang/config.toml")
        .output()
        .expect("spawn webfang");

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(i32::from(EXIT_CONFIG)),
        "a relative override must exit {EXIT_CONFIG}.\nstderr:\n{stderr}"
    );
    // Deliberately NOT dir-redacted: there is no temp dir here, and handing the
    // redactor a dummy dir would rewrite the `.` inside `config.toml` into
    // `<OUT_DIR>`, baking that artifact into the committed snapshot.
    assert_plain_stderr_snapshot("override_is_relative", &stderr);
}

// ---------------------------------------------------------------------------
// The accepted cases — the contract must not over-reject
// ---------------------------------------------------------------------------

/// An EMPTY existing config yields exactly the defaults the old absent file
/// produced. This is the equivalence the shared behavioral harness now relies
/// on: it points every spawned binary at an empty config precisely because an
/// absent one is no longer an acceptable override. If this test ever fails,
/// every test in the binary suite is broken.
#[tokio::test]
async fn empty_explicit_override_runs_with_defaults() {
    let t = BehavioralTest::new().await;
    mount_discovery(&t).await;

    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, "").expect("write empty config");

    let out = t
        .scraper_cmd()
        .arg("--dry-run")
        .env("WEBFANG_CONFIG", &config_path)
        .output()
        .expect("spawn webfang");

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "an empty override must be accepted and run with defaults.\nstderr:\n{stderr}"
    );
}

/// A populated override is accepted. That its values are APPLIED is pinned by
/// `budget_override_test`; this pins only that the hardened loader does not
/// over-reject a good file.
#[tokio::test]
async fn valid_explicit_override_is_accepted() {
    let t = BehavioralTest::new().await;
    mount_discovery(&t).await;

    let tmp = tempfile::TempDir::new().expect("create temp dir");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, "max_pages = 5\nformat = \"markdown\"\n")
        .expect("write valid config");

    let out = t
        .scraper_cmd()
        .arg("--dry-run")
        .env("WEBFANG_CONFIG", &config_path)
        .output()
        .expect("spawn webfang");

    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert_eq!(
        out.status.code(),
        Some(0),
        "a valid override must be accepted.\nstderr:\n{stderr}"
    );
}
