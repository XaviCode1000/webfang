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
#[cfg(unix)]
use std::time::Instant;

use common::cli_harness::{mock_sitemap, BehavioralTest};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
#[cfg(unix)]
use wiremock::MockServer;
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

/// Per-page response delay for the `during_cancel_drain` row: slows the
/// crawl enough that the SIGINT always lands mid-run (the
/// `signal_lifecycle_test` pattern — the signal moment is pinned by
/// wiremock's request log, the delay only widens the window).
#[cfg(unix)]
const PAGE_DELAY: Duration = Duration::from_millis(200);

/// Poll interval while waiting for request counts and child exit.
#[cfg(unix)]
const POLL: Duration = Duration::from_millis(50);

/// How long the signalled child may take to die before the test fails.
#[cfg(unix)]
const EXIT_TIMEOUT: Duration = Duration::from_secs(60);

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

/// Count GETs to real content pages (anything that is not `/robots.txt` and
/// not the sitemap) — the deterministic signal trigger for the
/// `during_cancel_drain` row (the `signal_lifecycle_test` pattern).
#[cfg(unix)]
async fn count_page_fetches(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("request log")
        .iter()
        .filter(|r| {
            let p = r.url.path();
            p != "/robots.txt" && !p.ends_with("sitemap.xml")
        })
        .count()
}

/// Poll until the server has seen at least `k` page fetches (deterministic
/// signal trigger), bounded by [`EXIT_TIMEOUT`].
#[cfg(unix)]
async fn wait_for_page_fetches(server: &MockServer, k: usize) {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        if count_page_fetches(server).await >= k {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "server never reached {k} page fetches — crawl stalled"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Spawn the real binary as a bare `std::process::Command` with piped
/// stderr and the crash point armed, replicating the harness's hermetic env
/// (no `WEBFANG_*`/AI-model poisoning, fresh `XDG_CACHE_HOME`) — needed
/// because the row must hold the child handle to deliver the SIGINT
/// (the `signal_lifecycle_test::spawn_webfang` pattern).
#[cfg(unix)]
fn spawn_crash_armed_child(args: &[String], cache_dir: &Path, point: &str) -> std::process::Child {
    use std::process::Stdio;

    let mut c = std::process::Command::new(common::cli_harness::webfang_path());
    c.args(args).stdout(Stdio::null()).stderr(Stdio::piped());
    for (k, _) in std::env::vars() {
        if k.starts_with("WEBFANG_") || k == "AI_MODEL_ID" {
            c.env_remove(&k);
        }
    }
    // Hermetic config, same reason as `sanitize_env` in the shared harness:
    // the strip above removes the developer's `WEBFANG_CONFIG`, and without a
    // replacement the child falls back to `~/.config/webfang/config.toml`. A
    // real file enabling `clean-ai` would route the export through the AI
    // drain, which is exactly a path where this row's armed point must still
    // fire; an empty file means the all-`None` defaults instead.
    let hermetic_config = cache_dir.join("hermetic-config.toml");
    if !hermetic_config.exists() {
        std::fs::write(&hermetic_config, "").expect("write hermetic config.toml");
    }
    c.env("WEBFANG_CONFIG", &hermetic_config);
    c.env("XDG_CACHE_HOME", cache_dir);
    // SSRF entry-guard allowance (F-06 + F-32, #1217): the wiremock mock
    // binds 127.0.0.1, a forbidden literal — disarm ONLY the entry layer.
    c.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    // Armed AFTER the WEBFANG_* strip so the parent process can never
    // poison the armed point.
    c.env(webfang_core::cli::crash_points::ENV_VAR, point);
    c.spawn().expect("spawn crash-armed child")
}

/// Send SIGINT to `child` via the system `kill` binary (the crate forbids
/// `unsafe`, so no direct `libc::kill`; the `signal_lifecycle_test` pattern).
#[cfg(unix)]
fn send_sigint(child: &std::process::Child, what: &str) {
    let status = std::process::Command::new("kill")
        .arg("-INT")
        .arg(child.id().to_string())
        .status()
        .unwrap_or_else(|e| panic!("{what}: invoking kill failed: {e}"));
    assert!(
        status.success(),
        "{what}: kill -INT {} must succeed",
        child.id()
    );
}

/// A `Child` that dies with its handle (the `signal_lifecycle_test`
/// `KillOnDrop` pattern, minimal form): a test that panics between spawn and
/// wait must not leak a running child — nextest would report it LEAKY and
/// bury the real failure under process noise.
#[cfg(unix)]
struct KillOnDrop(std::process::Child);

#[cfg(unix)]
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Disarm BEFORE signalling: a reaped child's PID is free for reuse,
        // so an unconditional kill could SIGKILL an unrelated process.
        if self.0.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }
        let _ = self.0.kill();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.0.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::sleep(POLL),
            }
        }
    }
}

#[cfg(unix)]
impl std::ops::Deref for KillOnDrop {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(unix)]
impl std::ops::DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Wait for the signalled child to die; fail (after SIGKILL) if it exceeds
/// [`EXIT_TIMEOUT`] — a drain death must not hang. Returns the exit status
/// plus the captured stderr (the panic material for the SIGKILL assertion).
#[cfg(unix)]
fn wait_child_exit(mut child: KillOnDrop, what: &str) -> (ExitStatus, String) {
    use std::io::Read;

    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                // The child is reaped and its stderr write-end is closed, so
                // draining the pipe to EOF cannot block.
                let mut stderr = String::new();
                child
                    .stderr
                    .take()
                    .expect("stderr pipe")
                    .read_to_string(&mut stderr)
                    .unwrap_or_default();
                return (status, stderr);
            },
            None => {
                assert!(
                    Instant::now() < deadline,
                    "{what}: child did not exit within {EXIT_TIMEOUT:?} — shutdown hung"
                );
                std::thread::sleep(POLL);
            },
        }
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
    let child = spawn_crash_armed_child(&args, cache_dir.path(), DURING_CANCEL_DRAIN);
    let child = KillOnDrop(child);
    wait_for_page_fetches(&t.server, 1).await;
    send_sigint(&child, DURING_CANCEL_DRAIN);
    let (crashed_status, crashed_stderr) = wait_child_exit(child, DURING_CANCEL_DRAIN);
    assert_killed_by_sigkill(&crashed_status, &crashed_stderr);

    // Attempt 2: same state dir, crash env and signal REMOVED, resume gate
    // active — the same contract as every other row.
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
