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
use std::time::Duration;

use common::cli_harness::{
    mock_sitemap, send_signal, spawn_webfang_with, wait_exit, wait_for_page_fetches,
    BehavioralTest, PAGE_DELAY,
};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

use webfang_core::cli::crash_points::{
    DURING_CANCEL_DRAIN, MID_EXTRACTION, MID_FETCH, MID_JSONL_LINE, MID_STATE_FILE_WRITE,
    POST_EXTRACTION_PRE_PIPELINE, POST_FETCH_PRE_EXTRACT, POST_FLUSH_PRE_COMMIT, PRE_FIRST_PERSIST,
    TMP_WRITTEN_PRE_RENAME, WHILE_HOLDING_LOCK,
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
    mount_crash_fixture_delayed(t, Duration::ZERO).await
}

/// Same fixture with a per-page response delay — the `during_cancel_drain`
/// row needs the crawl slow enough for a deterministic mid-run SIGINT.
async fn mount_crash_fixture_delayed(t: &BehavioralTest, delay: Duration) -> String {
    for (idx, page) in PAGES.iter().enumerate() {
        Mock::given(method("GET"))
            .and(path(*page))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><article><h1>Crash Page {idx}</h1><p>Distinct substantive body number {idx} long enough to pass minimum-content guards.</p></article></body></html>",
            )).set_delay(delay))
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

/// Attempt 2 of every row: rerun over the SAME state dir with the crash env
/// and the signal removed, and assert invariant (d) — the resume exits 0.
///
/// Shared by the plain rows and by `during_cancel_drain`, which differ only in
/// how attempt 1 killed the child.
fn resume_run(t: &BehavioralTest, state_dir: &Path, sitemap_url: &str) {
    let resumed = t
        .state_dir_cmd(state_dir)
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(sitemap_url)
        .arg("--quiet")
        .output()
        .expect("spawn resume run");
    assert!(
        resumed.status.success(),
        "invariant (d): resume rerun must exit 0: {}",
        String::from_utf8_lossy(&resumed.stderr)
    );
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
    resume_run(&t, state_dir.path(), &sitemap_url);

    assert_store_records_complete(state_dir.path());
    assert_jsonl_exactly_once(&t);
}

// ---------------------------------------------------------------------------
// Matrix rows — one per pinned crash point.
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

/// `during_cancel_drain` (#2244 decision A): the crash point sits in the
/// `Cancelled` branch of the export drain, so it is reachable ONLY after a
/// cancellation token has been observed — this row cannot be a plain
/// `crash_row(point)` over a normally running process. It spawns the child,
/// delivers SIGINT at the deterministic moment wiremock observed the first
/// page request (never a sleep — the `signal_lifecycle_test` pattern), and
/// asserts the same crash → resume → four-invariants contract as every row:
/// the completed item is persisted, the cancelled one is not, and the rerun
/// loses no URL and duplicates no `checksum_sha256`.
///
/// Unix-only: the SIGINT delivery cannot reach a child process on Windows
/// (#1631), so the row measures the harness instead of the product there.
#[cfg(unix)]
#[tokio::test]
async fn crash_during_cancel_drain_sigint_drain_kills_then_recovers() {
    let t = BehavioralTest::new().await;
    let sitemap_url = mount_crash_fixture_delayed(&t, PAGE_DELAY).await;
    let state_dir = TempDir::new().expect("state tempdir");
    // Hermetic cache for the spawned children (mirrors sanitize_env).
    let cache_dir = TempDir::new().expect("cache tempdir");

    // Attempt 1: armed crash point + SIGINT mid-scrape. The crawl drains
    // its in-flight page, the export starts over the cancelled token, the
    // first item completes its D3 sequence, and the drain-observed
    // cancellation arms DURING_CANCEL_DRAIN → SIGKILL.
    let args: Vec<String> = vec![
        "--resume".to_string(),
        "--state-dir".to_string(),
        state_dir.path().display().to_string(),
        "--url".to_string(),
        t.server.uri(),
        "--output".to_string(),
        t.out.path().display().to_string(),
        "--use-sitemap".to_string(),
        "--sitemap-url".to_string(),
        sitemap_url.clone(),
        "--quiet".to_string(),
    ];
    let child = spawn_webfang_with(&args, cache_dir.path(), DURING_CANCEL_DRAIN, |c| {
        // stderr is the panic material for the SIGKILL assertion; stdout is
        // noise this row never reads.
        c.stdout(std::process::Stdio::null());
        // Armed AFTER the harness's `WEBFANG_*` strip, so the parent process
        // can never poison the armed point.
        c.env(
            webfang_core::cli::crash_points::ENV_VAR,
            DURING_CANCEL_DRAIN,
        );
    });
    wait_for_page_fetches(&t.server, 1).await;
    send_signal(&child, "INT", DURING_CANCEL_DRAIN);
    let (crashed_status, crashed_stderr) = wait_exit(child, DURING_CANCEL_DRAIN);
    assert_killed_by_sigkill(&crashed_status, &crashed_stderr);

    // Attempt 2: same state dir, crash env and signal REMOVED, resume gate
    // active — the same contract as every other row.
    resume_run(&t, state_dir.path(), &sitemap_url);

    assert_store_records_complete(state_dir.path());
    assert_jsonl_exactly_once(&t);
}
