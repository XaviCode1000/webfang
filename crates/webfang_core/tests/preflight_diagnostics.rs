//! Exit-code contract for preflight validation, plus the exit-code table pin.
//!
//! Two contracts, both behavioral:
//!
//! 1. **An invalid burst fails closed.** `--rate-limit-burst` accepts exactly
//!    one shape: a `u32` >= 1. `#1813` closed the last fail-open arm, so a
//!    garbage value now stops the run with a Spanish `CliExit::ConfigError`
//!    (exit 78) instead of degrading to the derived default. Pinned from the
//!    outside: the run FAILS, stderr names the offending raw value, and the
//!    validation message appears at DEFAULT verbosity with no `-v` flag. A
//!    valid `--rate-limit-burst 4` still succeeds silently (negative control
//!    against vacuous matching).
//!
//!    History: this contract used to be the opposite — `abc` degraded to the
//!    default and a `preflight_notes` WARN announced the substitution (#1431,
//!    which had to exist because the notice was emitted before the tracing
//!    subscriber was installed and would have been dropped). Failing closed
//!    removes the need for the notice entirely, so that replay path has no
//!    producer left.
//!
//! 2. **Exit-code table cannot drift.** The `--help` `EXIT CODES` table and
//!    the `docs/src/cli-reference.md` mirror must name every `EXIT_*`
//!    constant in `webfang_core::cli::error`. Asserted programmatically
//!    against the constants, not via a full `--help` snapshot.
//!
//! Posture: the shared `cmd()` harness (entry layer disarmed for the
//! loopback fixture, child env only — no global mutation). Determinism:
//! content assertions only, no timing, no sleeps.

#[path = "common/mod.rs"]
mod common;

use common::cli_harness::cmd;
use common::fixture_server::start_fixture;
use webfang_core::cli::error as cli_error;

const RUN_TIMEOUT_SECS: u64 = 120;

/// Distinctive fragment of the burst-rejection message.
///
/// Chosen to NOT match the routine `scrape rate limiter wired … burst: N`
/// line, which also mentions "burst" on every run: the assertion must fire on
/// the validation error, never on the wiring summary.
const INVALID_FRAGMENT: &str = "no es un número";

/// Run a single-page scrape of `url` with an explicit burst flag, returning
/// the completed output.
fn scrape_with_burst(url: &str, out: &tempfile::TempDir, burst: &str) -> std::process::Output {
    cmd()
        .arg("--url")
        .arg(url)
        .arg("--single-page")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(out.path())
        .arg("--rate-limit-burst")
        .arg(burst)
        .arg("--timeout-secs")
        .arg("10")
        .timeout(std::time::Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang")
}

/// #1813: a garbage burst FAILS CLOSED with the Spanish validation error at
/// default verbosity, naming the offending raw value. Before this, the same
/// input degraded to the hardware-derived default and the only evidence was one
/// WARN line — the operator got a different request cadence than they asked for.
#[tokio::test]
async fn garbage_burst_fails_closed_with_spanish_error() {
    let (base, _log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    let output = scrape_with_burst(&format!("{base}/ok"), &out, "abc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(78),
        "a garbage burst must fail closed (ConfigError), not degrade: exit {:?}\nstderr: {stderr}",
        output.status.code()
    );
    assert!(
        stderr.contains(INVALID_FRAGMENT),
        "stderr must carry the burst-validation message, got:\n{stderr}"
    );
    assert!(
        stderr.contains("abc"),
        "the error must name the offending raw value, got:\n{stderr}"
    );
}

/// Negative control: a valid numeric burst is still honoured silently — no
/// validation error may appear. Guards the fail-closed change against a
/// false positive that rejected everything.
#[tokio::test]
async fn numeric_burst_is_honoured_without_validation_error() {
    let (base, _log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    let output = scrape_with_burst(&format!("{base}/ok"), &out, "4");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a numeric burst must succeed: exit {:?}\nstderr: {stderr}",
        output.status.code()
    );
    assert!(
        !stderr.contains(INVALID_FRAGMENT),
        "a valid burst must not produce a validation error, got:\n{stderr}"
    );
}

/// Every `EXIT_*` constant in `cli::error` must be named in the `--help`
/// `EXIT CODES` table. Asserted against the constants themselves: adding a
/// new exit constant without documenting it (or dropping a documented code)
/// fails this test without snapshotting unrelated help text.
#[test]
fn help_exit_table_lists_every_exit_constant() {
    let table = help_exit_codes_table();
    let codes: &[u8] = &[
        cli_error::EXIT_SUCCESS,
        cli_error::EXIT_EMPTY_DISCOVERY,
        cli_error::EXIT_SCRAPER_FAILURE,
        cli_error::EXIT_USAGE_ERROR,
        cli_error::EXIT_DATA_ERROR,
        cli_error::EXIT_UNAVAILABLE,
        cli_error::EXIT_IO_ERROR,
        cli_error::EXIT_PROTOCOL,
        cli_error::EXIT_FORBIDDEN,
        cli_error::EXIT_CONFIG,
    ];
    for code in codes {
        assert!(
            table_contains_code(&table, *code),
            "EXIT CODES table must document exit {code}, got:\n{table}"
        );
    }
}

/// The `docs/src/cli-reference.md` mirror must carry the same codes, so the
/// two published tables cannot rot independently of each other.
#[test]
fn docs_exit_table_mirrors_every_exit_constant() {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let doc_path = manifest.join("../../docs/src/cli-reference.md");
    let doc = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", doc_path.display()));
    let codes: &[u8] = &[
        cli_error::EXIT_SUCCESS,
        cli_error::EXIT_EMPTY_DISCOVERY,
        cli_error::EXIT_SCRAPER_FAILURE,
        cli_error::EXIT_USAGE_ERROR,
        cli_error::EXIT_DATA_ERROR,
        cli_error::EXIT_UNAVAILABLE,
        cli_error::EXIT_IO_ERROR,
        cli_error::EXIT_PROTOCOL,
        cli_error::EXIT_FORBIDDEN,
        cli_error::EXIT_CONFIG,
    ];
    for code in codes {
        assert!(
            table_contains_code(&doc, *code),
            "cli-reference.md must document exit {code}"
        );
    }
}

/// Run `webfang --help` and return the `EXIT CODES:` section only, so the
/// assertions cannot pass vacuously on a number mentioned elsewhere.
fn help_exit_codes_table() -> String {
    let output = cmd()
        .arg("--help")
        .timeout(std::time::Duration::from_secs(RUN_TIMEOUT_SECS))
        .output()
        .expect("spawn webfang --help");
    assert!(output.status.success(), "`webfang --help` must exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let start = stdout
        .find("EXIT CODES:")
        .expect("--help must contain an EXIT CODES section");
    stdout[start..].to_owned()
}

/// True when `table` contains a line whose first token is `code` — i.e. the
/// code is documented as a table entry, not mentioned in passing prose.
fn table_contains_code(table: &str, code: u8) -> bool {
    let needle = code.to_string();
    table.lines().any(|line| {
        line.split_whitespace()
            .next()
            .is_some_and(|first| first == needle)
    })
}
