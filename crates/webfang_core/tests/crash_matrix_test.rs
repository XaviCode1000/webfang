//! SIGKILL crash matrix harness (design D6, PR5a).
//!
//! One test per pinned crash point. Each row:
//!
//! 1. spawns the real `webfang` binary with `WEBFANG_CRASH_AT=<point>` over a
//!    small wiremock site (2 pages via sitemap) and an explicit `--state-dir`,
//! 2. asserts the child died by SIGNAL (SIGKILL), never by a clean exit,
//! 3. reruns WITH `--resume` and the same state dir (crash env removed),
//! 4. asserts the FOUR global invariants AFTER the resume rerun completes:
//!    (a) every input URL is present in the store records — 0 lost;
//!    (b) output JSONL parses line-by-line and every `checksum_sha256` is
//!    EXACTLY once (torn tails fixed, no duplicated committed lines);
//!    (c) every persisted record passes the D2 validation table (`load`
//!    succeeds; committed records carry hash + location, no last_error);
//!    (d) the resume rerun exits successfully.
//!
//! Run with: `cargo nextest run --test crash_matrix_test`

#[path = "common/mod.rs"]
mod common;

use std::path::Path;
use std::process::ExitStatus;

use common::cli_harness::{mock_sitemap, BehavioralTest};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use webfang_core::cli::crash_points::{
    MID_EXTRACTION, MID_FETCH, MID_JSONL_LINE, MID_STATE_FILE_WRITE, POST_EXTRACTION_PRE_PIPELINE,
    POST_FETCH_PRE_EXTRACT, POST_FLUSH_PRE_COMMIT, PRE_FIRST_PERSIST, TMP_WRITTEN_PRE_RENAME,
    WHILE_HOLDING_LOCK,
};
use webfang_core::domain::page_state::PageStatus;
use webfang_core::infrastructure::export::RecordStore;

/// Host (no port) that `domain_from_url` derives from the wiremock origin —
/// the state file is named `<domain>.json`.
const DOMAIN: &str = "127.0.0.1";

/// The exact input URLs the fixture site advertises.
const PAGES: [&str; 2] = ["/page1", "/page2"];

/// Mount two distinct pages plus a sitemap listing exactly those URLs.
/// Distinct bodies keep every `checksum_sha256` unique so invariant (b)
/// detects both duplication and loss.
async fn mount_crash_fixture(t: &BehavioralTest) -> String {
    for (idx, page) in PAGES.iter().enumerate() {
        Mock::given(method("GET"))
            .and(path(*page))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><article><h1>Crash Page {idx}</h1><p>Distinct substantive body number {idx} long enough to pass minimum-content guards.</p></article></body></html>",
            )))
            .mount(&t.server)
            .await;
    }

    let base = t.server.uri();
    let sitemap_url = format!("{base}/sitemap.xml");
    let entries: String = PAGES
        .iter()
        .map(|p| format!("<url><loc>{base}{p}</loc></url>"))
        .collect();
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{entries}</urlset>"#
    );
    mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;
    sitemap_url
}

/// Assert the child process was killed by SIGKILL (signal death, not a clean
/// exit code and not some other signal).
///
/// `child_stderr` travels in the panic message so a death by a DIFFERENT
/// signal names its own cause instead of leaving a bare wait status — which is
/// how the one real case was found: `mid_state_file_write` died by `SIGABRT`
/// on `macos-latest` because `kill_self` fell through from a successful `kill`
/// into `abort()` before the pending `SIGKILL` was delivered (#2063). The
/// assertion stays strict at `SIGKILL(9)` — that is the contract — and the
/// harness now waits for its own signal and logs the errno if it refuses one.
fn assert_killed_by_sigkill(status: &ExitStatus, child_stderr: &str) {
    assert!(
        !status.success(),
        "armed crash point must terminate the child by signal, got clean exit"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            status.signal(),
            Some(9),
            "child must die by SIGKILL(9); exit code {status:?}; child stderr: {child_stderr}"
        );
    }
}

/// Invariant (b): every JSONL line parses and every content hash appears
/// EXACTLY once (torn tails fixed, no duplicated committed lines);
fn assert_jsonl_exactly_once(t: &BehavioralTest) {
    let files = t.find_files("jsonl");
    assert!(!files.is_empty(), "resume must produce JSONL export");
    let mut seen = std::collections::HashMap::new();
    let mut total = 0usize;
    for file in &files {
        let content = std::fs::read(file).expect("read jsonl");
        assert!(
            content.ends_with(b"\n"),
            "{} must end on a newline boundary after recovery",
            file.display()
        );
        let text = String::from_utf8(content).expect("utf8 jsonl");
        for line in text.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON line");
            let hash = value
                .get("checksum_sha256")
                .and_then(serde_json::Value::as_str)
                .expect("line carries checksum_sha256")
                .to_owned();
            *seen.entry(hash).or_insert(0usize) += 1;
            total += 1;
        }
    }
    let duplicates: Vec<_> = seen.iter().filter(|(_, &n)| n > 1).collect();
    assert!(
        duplicates.is_empty(),
        "no committed line may appear twice, found duplicates: {duplicates:?}"
    );
    assert_eq!(total, PAGES.len(), "exactly one line per input page");
}

/// Invariants (a)+(c): after resume, the record store loads cleanly, holds a
/// record for EVERY input URL, and each record passes the D2 validation table
/// at its terminal COMMITTED state (hash + output_location present, no
/// `last_error`; any quarantined record would surface as a missing URL here).
fn assert_store_records_complete(state_dir: &Path) {
    let store = RecordStore::new(DOMAIN).with_state_dir(state_dir.to_path_buf());
    let records = store
        .load()
        .expect("store load must succeed without corruption");
    assert_eq!(
        records.len(),
        PAGES.len(),
        "every input URL must be present exactly once in the store"
    );
    for page in PAGES {
        let record = records
            .values()
            .find(|r| r.url.ends_with(page) || r.canonical_url.ends_with(page))
            .unwrap_or_else(|| panic!("record for {page} lost across crash+resume"));
        assert_eq!(
            record.status,
            PageStatus::Committed,
            "{page} must reach COMMITTED after resume"
        );
        assert!(record.content_hash.is_some(), "{page}: D2 requires hash");
        assert!(
            record.output_location.is_some(),
            "{page}: D2 requires output_location"
        );
        assert!(
            record.last_error.is_none(),
            "{page}: committed has no error"
        );
        assert!(
            record.attempts >= 1,
            "{page}: committed requires attempts >= 1"
        );
    }
}

/// Full matrix row: crash → resume → four global invariants.
async fn crash_row(point: &str) {
    let t = BehavioralTest::new().await;
    let sitemap_url = mount_crash_fixture(&t).await;
    let state_dir = TempDir::new().expect("state tempdir");

    // Attempt 1: armed crash point kills the child mid-pipeline.
    let crashed = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .env(webfang_core::cli::crash_points::ENV_VAR, point)
        .output()
        .expect("spawn crashed run");
    assert_killed_by_sigkill(&crashed.status, &String::from_utf8_lossy(&crashed.stderr));

    // Attempt 2: same state dir, crash env REMOVED, resume gate active.
    let resumed = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("spawn resume run");
    assert!(
        resumed.status.success(),
        "invariant (d): resume rerun must exit 0: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );

    assert_store_records_complete(state_dir.path());
    assert_jsonl_exactly_once(&t);
}

// ---------------------------------------------------------------------------
// Matrix rows — one per pinned crash point.
//
// `during_cancel_drain` has NO row, and that is a fact about the code, not
// an omission of this file: the point is armed in production
// (`application/export_factory.rs`, the `Cancelled` branch of the drain
// loop), but that branch is UNREACHABLE — the export session only observes
// a cancellation token when `ResumeContext::observing_cancel` is called, and
// in production nobody calls it (only the tests of `export_factory` do). So
// the session's `cancelled()` is always false, the drain branch never runs,
// and there is nothing for a row to trigger. The wiring that would make it
// reachable is a production change (the CLI's export flow has to hand its
// cancellation token to the `ResumeContext`), and it is NOT this file's
// business. Until then, advertising a row here would be a row that cannot
// pass — a false green of the exact kind this suite exists to prevent.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crash_pre_first_persist_recovers_with_zero_loss() {
    crash_row(PRE_FIRST_PERSIST).await;
}

#[tokio::test]
async fn crash_mid_fetch_recovers_with_zero_loss() {
    crash_row(MID_FETCH).await;
}

#[tokio::test]
async fn crash_post_fetch_pre_extract_recovers_with_zero_loss() {
    crash_row(POST_FETCH_PRE_EXTRACT).await;
}

#[tokio::test]
async fn crash_mid_extraction_recovers_with_zero_loss() {
    crash_row(MID_EXTRACTION).await;
}

#[tokio::test]
async fn crash_post_extraction_pre_pipeline_recovers_with_zero_loss() {
    crash_row(POST_EXTRACTION_PRE_PIPELINE).await;
}

#[tokio::test]
async fn crash_mid_jsonl_line_torn_tail_is_repaired() {
    crash_row(MID_JSONL_LINE).await;
}

#[tokio::test]
async fn crash_post_flush_pre_commit_never_duplicates_lines() {
    crash_row(POST_FLUSH_PRE_COMMIT).await;
}

#[tokio::test]
async fn crash_while_holding_lock_releases_and_recovers() {
    crash_row(WHILE_HOLDING_LOCK).await;
}

#[tokio::test]
async fn crash_tmp_written_pre_rename_discards_tmp_and_recovers() {
    crash_row(TMP_WRITTEN_PRE_RENAME).await;
}

#[tokio::test]
async fn crash_mid_state_file_write_truncated_tmp_is_gced() {
    crash_row(MID_STATE_FILE_WRITE).await;
}

// ---------------------------------------------------------------------------
// #2066 — a DISCARDED SIGKILL must not hang the child.
//
// The kernel DISCARDS a `SIGKILL` sent from inside its own PID namespace to
// that namespace's init (the PID-1 special case): `kill()` returns 0 and the
// signal NEVER arrives. The pre-fix `kill_self` sent the signal and then
// waited for it (`loop { libc::pause() }`), so the process hung forever and
// the crash-matrix lane burned nextest's slow-timeout instead of failing with
// a diagnostic that says what happened.
//
// The reproduction is the environment itself: the child is launched as the
// init of a PID namespace, which is the only way to get `getpid() == 1`
// without being the real init of the machine.
//
// The three assertions are the ones the issue asks for: the child does NOT
// hang (it finishes inside the window), it does NOT exit successfully, and
// its stderr carries the diagnostic.
// ---------------------------------------------------------------------------

/// The window the child has to finish in. It is generous (the child does no
/// real work) and it is FINITE on purpose: a test that waits forever for a
/// process that must not hang is the same hang, one layer up.
#[cfg(target_os = "linux")]
const DISCARDED_SIGKILL_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg(target_os = "linux")]
#[tokio::test]
async fn crash_point_never_hangs_when_sigkill_is_discarded() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let t = BehavioralTest::new().await;
    let sitemap_url = mount_crash_fixture(&t).await;
    let state_dir = TempDir::new().expect("state tempdir");

    // The matrix command, launched as the init of a NEW PID namespace:
    // `unshare` propagates the child's exit status, and the child's
    // `getpid()` is 1 inside that namespace.
    let mut cmd = Command::new("unshare");
    cmd.arg("--user")
        .arg("--map-root-user")
        .arg("--fork")
        .arg("--pid")
        .arg("--")
        .arg(common::cli_harness::webfang_path())
        .arg("--resume")
        .arg("--state-dir")
        .arg(state_dir.path())
        .arg("--url")
        .arg(t.server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // The same hermetic treatment every other spawned binary gets, in the
    // same ORDER: first the poisoned env vars are stripped (every
    // WEBFANG_* and AI_MODEL_ID, so a CI bug-discovery workflow cannot
    // decide what this child does), and only THEN this test's own variables
    // are set — the crash point among them, which a strip done afterwards
    // would remove.
    for (key, _) in std::env::vars() {
        if key.starts_with("WEBFANG_") || key == "AI_MODEL_ID" {
            cmd.env_remove(&key);
        }
    }
    // The SSRF entry guard is disarmed: the fixture binds 127.0.0.1, a
    // forbidden literal for the production path. The cache and the config
    // belong to the test, not to the operator's machine. Without this the
    // child dies of a config or SSRF error and never reaches the crash
    // point — the test would then prove nothing.
    cmd.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    cmd.env("XDG_CACHE_HOME", common::cli_harness::hermetic_cache_dir());
    cmd.env(
        "WEBFANG_CONFIG",
        common::cli_harness::hermetic_empty_config_file(),
    );
    cmd.env(webfang_core::cli::crash_points::ENV_VAR, PRE_FIRST_PERSIST);

    let mut child = cmd.spawn().expect("spawn the crash child");

    // Bounded wait: the child must END inside the window. A child that is
    // still running when the window closes is the defect itself, so the
    // failure says exactly that (and not "the test timed out").
    let deadline = std::time::Instant::now() + DISCARDED_SIGKILL_WINDOW;
    let status = loop {
        match child.try_wait().expect("poll the crash child") {
            Some(status) => break status,
            None => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!(
                        "the crash child did not finish within {}s: the SIGKILL was \
                         discarded and the child hung instead of dying (issue #2066)",
                        DISCARDED_SIGKILL_WINDOW.as_secs()
                    );
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            },
        }
    };

    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }

    // The namespace could not be created at all: that is an ENVIRONMENT
    // failure, and it must not read as a pass. The remediation is named so
    // the reader knows what to fix.
    assert!(
        !stderr.contains("unshare failed"),
        "the PID namespace could not be created, so the discarded-SIGKILL path was never \
         exercised; enable unprivileged user namespaces (e.g. `sysctl -w \
         kernel.unprivileged_userns_clone=1`) or run this test as root. unshare said: \
         {stderr}"
    );

    assert!(
        !status.success(),
        "the crash child must NOT exit successfully when the SIGKILL is discarded; \
         status {status:?}; stderr: {stderr}"
    );
    assert!(
        stderr.contains("SIGKILL was discarded"),
        "the child must die with a diagnostic that NAMES the discard; stderr: {stderr}"
    );
}
