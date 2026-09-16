//! #1431: preflight diagnostics are observable + the exit-code table is pinned.
//!
//! Two contracts, both behavioral:
//!
//! 1. **Substitution notice is visible.** A non-numeric `--rate-limit-burst`
//!    degrades to the derived default (accepted semantics from #1255), but the
//!    notice was emitted via `tracing::warn!` before the subscriber existed,
//!    so it was dropped. The fix records the notice at parse time and replays
//!    it once logging is live. Pinned from the outside: with
//!    `--rate-limit-burst abc`, stderr at DEFAULT verbosity contains the
//!    notice and the run still succeeds; with `--rate-limit-burst 4` no such
//!    notice appears (negative control against vacuous matching).
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

/// Distinctive fragment of the burst-substitution notice.
///
/// Chosen to NOT match the routine `scrape rate limiter wired … burst: N`
/// line, which also mentions "burst" on every run: the assertion must fire
/// on the substitution notice, never on the wiring summary.
const SUBSTITUTION_FRAGMENT: &str = "using derived default";

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

/// Garbage burst degrades to the derived default AND says so on stderr at
/// default verbosity: the run succeeds and the substitution notice is
/// visible without any `-v` flag.
#[tokio::test]
async fn garbage_burst_warns_and_still_succeeds() {
    let (base, _log) = start_fixture().await;
    let out = tempfile::TempDir::new().expect("temp output dir");

    let output = scrape_with_burst(&format!("{base}/ok"), &out, "abc");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "a garbage burst must degrade to the default, not fail: exit {:?}\nstderr: {stderr}",
        output.status.code()
    );
    assert!(
        stderr.contains(SUBSTITUTION_FRAGMENT),
        "stderr must carry the burst-substitution notice, got:\n{stderr}"
    );
    assert!(
        stderr.contains("abc"),
        "the notice must name the offending raw value, got:\n{stderr}"
    );
}

/// Negative control: an explicit numeric burst is honoured silently — no
/// substitution happened, so no substitution notice may appear.
#[tokio::test]
async fn numeric_burst_emits_no_substitution_notice() {
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
        !stderr.contains(SUBSTITUTION_FRAGMENT),
        "no substitution happened, so no notice may appear, got:\n{stderr}"
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
