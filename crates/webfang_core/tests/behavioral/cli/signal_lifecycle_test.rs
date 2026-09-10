//! Interruption/resume lifecycle E2E evidence (issue #1292, ADR-0016 §5).
//!
//! One named test per finding, driven through the REAL `webfang` binary:
//!
//! - **P8-4** — `resume_after_completion_is_idempotent`: a completed crawl
//!   followed by `--resume` re-fetches ZERO pages and exits 0.
//! - **P8-5** — `sigint_shutdown_is_resumable` / `sigterm_shutdown_is_resumable`:
//!   a real signal delivered at a DETERMINISTIC moment (after the k-th request
//!   observed by wiremock — the crash-matrix pattern) exits 0 gracefully
//!   (#509 semantics), persists state, and the resumed run completes every
//!   remaining page without re-fetching anything already committed.
//! - **F-39** — `sigint_checkpoint_frontier_is_bounded`: SIGINT during a
//!   link-dense DOM crawl persists a checkpoint whose queued frontier is
//!   bounded by the run's OWN `max_pages`, never the whole discovered set,
//!   and the resumed run does not replay a foreign frontier.
//!
//! Determinism: the signal moment is pinned by wiremock's request log, not by
//! sleeps; wiremock delays only slow the crawl so the signal lands mid-run.

use crate::BehavioralTest;
use std::collections::BTreeMap;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// How long a child may take to exit after the signal before the test fails.
const EXIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll interval while waiting for request counts and child exit.
const POLL: Duration = Duration::from_millis(50);

/// Per-page response delay: slows the crawl enough that the signal always
/// lands mid-run, without making the test slow.
const PAGE_DELAY: Duration = Duration::from_millis(200);

/// Body that clears the 50-char minimum-content guard.
fn article_body(title: &str) -> String {
    format!(
        "<html><body><article><h1>{title}</h1><p>{title} carries enough substantive text to clear the minimum content guard comfortably.</p></article></body></html>"
    )
}

/// Mount `n` article pages, each with [`PAGE_DELAY`], and return their paths.
async fn mount_delayed_pages(server: &MockServer, n: usize) -> Vec<String> {
    let mut paths = Vec::with_capacity(n);
    for i in 0..n {
        let p = format!("/page-{i}");
        Mock::given(method("GET"))
            .and(path(&p))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(article_body(&format!("Page {i}")))
                    .set_delay(PAGE_DELAY),
            )
            .mount(server)
            .await;
        paths.push(p);
    }
    paths
}

/// Count GETs to real content pages (anything that is not `/robots.txt` and
/// not the sitemap) — the "page fetch" metric for every assertion here.
async fn count_page_fetches(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| {
            let p = r.url.path();
            p != "/robots.txt" && !p.ends_with("sitemap.xml")
        })
        .count()
}

/// Per-path page fetch counts (robots/sitemap excluded).
async fn fetches_by_path(server: &MockServer) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for r in server.received_requests().await.unwrap() {
        let p = r.url.path().to_string();
        if p == "/robots.txt" || p.ends_with("sitemap.xml") {
            continue;
        }
        *map.entry(p).or_insert(0) += 1;
    }
    map
}

/// Poll until the server has seen at least `k` page fetches (deterministic
/// signal trigger), bounded by `deadline`.
async fn wait_for_page_fetches(server: &MockServer, k: usize) {
    let deadline = Instant::now() + Duration::from_secs(30);
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

/// Send `signal` to `child` via the system `kill` binary (the crate forbids
/// `unsafe`, so no direct `libc::kill`; the crash-matrix pattern's OS-level
/// trigger, expressed safely).
fn send_signal(child: &Child, signal: &str, what: &str) {
    let status = Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(child.id().to_string())
        .status()
        .unwrap_or_else(|e| panic!("{what}: invoking kill failed: {e}"));
    assert!(
        status.success(),
        "{what}: kill -{signal} {} must succeed",
        child.id()
    );
}

/// Wait for the child to exit on its own; fail (after SIGKILL) if it exceeds
/// [`EXIT_TIMEOUT`] — a graceful shutdown must not hang.
fn wait_exit(mut child: Child, what: &str) -> std::process::ExitStatus {
    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => return status,
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

/// Spawn the real binary as a bare `std::process::Command` with piped
/// output, replicating the harness's hermetic env (no `WEBFANG_*`/AI model
/// poisoning, fresh `XDG_CACHE_HOME`) — needed because the test must hold
/// the child handle to deliver the signal.
fn spawn_webfang(args: &[String], cache_dir: &std::path::Path, what: &str) -> Child {
    let mut c = Command::new(crate::common::webfang_path());
    c.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, _) in std::env::vars() {
        if k.starts_with("WEBFANG_") || k == "AI_MODEL_ID" {
            c.env_remove(&k);
        }
    }
    c.env("XDG_CACHE_HOME", cache_dir);
    // SSRF entry-guard allowance (F-06 + F-32, #1217): the wiremock mock binds
    // 127.0.0.1, a forbidden literal — the harness disarms ONLY the entry
    // layer for spawned binaries, exactly like `sanitize_env` does.
    c.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    c.spawn()
        .unwrap_or_else(|e| panic!("{what}: spawn failed: {e}"))
}

/// Reaped stdout+stderr of a spawned child.
fn output_of(mut child: Child) -> String {
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut out);
    }
    out
}

/// Mount a sitemap listing exactly `paths` and return the sitemap URL.
async fn mount_sitemap(server: &MockServer, paths: &[String]) -> String {
    let base = server.uri();
    let sitemap_url = format!("{base}/sitemap.xml");
    let items: String = paths
        .iter()
        .map(|p| format!("<url><loc>{base}{p}</loc></url>"))
        .collect();
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{items}</urlset>"#
        )))
        .mount(server)
        .await;
    sitemap_url
}

/// Build the sitemap-scrape argument vector over `state_dir`.
fn sitemap_args(
    state_dir: &std::path::Path,
    out_dir: &std::path::Path,
    base: &str,
    sitemap_url: &str,
) -> Vec<String> {
    [
        "--resume".to_string(),
        "--state-dir".to_string(),
        state_dir.display().to_string(),
        "--url".to_string(),
        base.to_string(),
        "--output".to_string(),
        out_dir.display().to_string(),
        "--use-sitemap".to_string(),
        "--sitemap-url".to_string(),
        sitemap_url.to_string(),
        "--quiet".to_string(),
    ]
    .to_vec()
}

// ---------------------------------------------------------------------------
// P8-4 — resume after completion is idempotent
// ---------------------------------------------------------------------------

/// A crawl that ran to completion, followed by `--resume` over the same state
/// dir, re-fetches ZERO pages, exits 0, and leaves the output untouched —
/// the completed state is the fixed point of the resume cycle (ADR-0016 §3,
/// "After completion" row).
#[tokio::test]
async fn p84_resume_after_completion_is_idempotent() {
    let t = BehavioralTest::new().await;
    let pages = mount_delayed_pages(&t.server, 3).await;
    let sitemap_url = mount_sitemap(&t.server, &pages).await;
    let state_dir = TempDir::new().unwrap();

    let run = |what: &'static str| {
        let out = t
            .state_dir_cmd(state_dir.path())
            .arg("--use-sitemap")
            .arg("--sitemap-url")
            .arg(&sitemap_url)
            .arg("--quiet")
            .output()
            .expect("run binary");
        assert!(
            out.status.success(),
            "{what} must exit 0, got {:?}: {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
    };

    run("run1");
    let fetches_after_run1 = count_page_fetches(&t.server).await;
    assert_eq!(
        fetches_after_run1, 3,
        "run1 fetches exactly the 3 sitemap pages"
    );
    let md_after_run1 = t.find_files("md").len();
    assert_eq!(md_after_run1, 3, "run1 exports all 3 pages");

    // The completed state: resume re-drives nothing at all.
    run("run2-resume");
    assert_eq!(
        count_page_fetches(&t.server).await,
        fetches_after_run1,
        "P8-4: resume after completion must fetch ZERO pages"
    );
    assert_eq!(
        t.find_files("md").len(),
        md_after_run1,
        "P8-4: resume after completion must not touch the output"
    );
}

// ---------------------------------------------------------------------------
// P8-5 — SIGINT/SIGTERM graceful shutdown is resumable
// ---------------------------------------------------------------------------

/// One signal case for the graceful-shutdown evidence: run the sitemap scrape,
/// deliver `signal` after the 3rd page fetch, and prove (1) exit 0 (cooperative
/// cancellation, #509 semantics), (2) fetched pages were committed to the
/// record store, (3) the resumed run finishes every remaining page without
/// re-fetching anything already committed.
async fn p85_signal_case(signal: &str, label: &'static str) {
    let t = BehavioralTest::new().await;
    let base = t.server.uri();
    let pages = mount_delayed_pages(&t.server, 8).await;
    let sitemap_url = mount_sitemap(&t.server, &pages).await;
    let state_dir = TempDir::new().unwrap();
    // Hermetic cache for the spawned children (mirrors sanitize_env).
    let cache = TempDir::new().unwrap();

    let args = sitemap_args(state_dir.path(), t.out.path(), &base, &sitemap_url);
    let spawn_run = |what: &'static str| spawn_webfang(&args, cache.path(), what);

    // Attempt 1: signal lands mid-run, deterministically after 3 page fetches.
    let child = spawn_run(label);
    wait_for_page_fetches(&t.server, 3).await;
    send_signal(&child, signal, label);
    let status = wait_exit(child, label);
    assert!(
        status.success(),
        "{label}: graceful shutdown must exit 0 (cooperative cancellation), got {status:?}"
    );

    let fetched_before = fetches_by_path(&t.server).await;
    let fetched_paths: Vec<&String> = fetched_before
        .iter()
        .filter(|(_, c)| **c > 0)
        .map(|(p, _)| p)
        .collect();
    assert!(
        fetched_paths.len() >= 3,
        "{label}: at least 3 pages must have been fetched before the signal"
    );

    // State persisted: the record store exists and every fetched page is
    // committed (SC-gate truth: only COMMITTED allows skip-on-resume).
    // Records live under the `StoreFile.records` envelope, keyed by canonical
    // URL (full origin incl. port).
    let state_file = state_dir.path().join("127.0.0.1.json");
    assert!(
        state_file.exists(),
        "{label}: record store must persist across the signal"
    );
    let records: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_file).unwrap()).unwrap();
    for p in &fetched_paths {
        let url = format!("{base}{p}");
        let record = &records["records"][&url];
        assert_eq!(
            record["status"], "COMMITTED",
            "{label}: {url} fetched before the signal must be COMMITTED"
        );
    }

    // Attempt 2: resume completes the rest without re-fetching anything.
    let resumed = spawn_run("resume run");
    let resume_status = wait_exit(resumed, "resume run");
    assert!(
        resume_status.success(),
        "{label}: resume run must exit 0, got {resume_status:?}"
    );

    let final_counts = fetches_by_path(&t.server).await;
    for p in &pages {
        let count = final_counts.get(p).copied().unwrap_or(0);
        assert_eq!(
            count, 1,
            "{label}: {p} must have been fetched EXACTLY once across both runs (got {count})"
        );
    }
    assert!(
        t.find_files("md").len() >= 8,
        "{label}: the resumed run completes the remaining pages"
    );
}

/// P8-5 — SIGINT mid-scrape shuts down gracefully and the state is resumable
/// with zero duplicate fetches (ADR-0016 §3 "After signal" row).
#[tokio::test]
async fn p85_sigint_shutdown_is_resumable() {
    p85_signal_case("INT", "SIGINT run").await;
}

/// P8-5 — SIGTERM behaves exactly like SIGINT: graceful drain, persisted
/// state, idempotent resume.
#[tokio::test]
async fn p85_sigterm_shutdown_is_resumable() {
    p85_signal_case("TERM", "SIGTERM run").await;
}

// ---------------------------------------------------------------------------
// F-39 — the SIGINT checkpoint frontier is bounded
// ---------------------------------------------------------------------------

/// SIGINT during a link-dense DOM crawl persists a checkpoint whose queued
/// frontier is bounded by the run's OWN `--max-pages` (never the whole
/// discovered set — the pathological 4955-queued-vs-49-visited shape), and
/// the resumed run does not replay a foreign frontier: every page is fetched
/// at most twice (once per run) and the resumed crawl stays inside its own
/// budget (ADR-0016 §3, checkpoint-as-scheduling-state).
#[tokio::test]
async fn f39_sigint_checkpoint_frontier_is_bounded() {
    const MAX_PAGES: usize = 5;

    let t = BehavioralTest::new().await;
    // Hub links 30 pages; only 5 may ever enter a checkpoint frontier.
    let mut links = String::new();
    for i in 0..30 {
        links.push_str(&format!("<a href=\"/deep-{i}\">Deep {i}</a>"));
    }
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(format!(
                "<html><body>{links}<p>This hub page carries enough substantive text to clear the minimum content guard comfortably.</p></body></html>"
            )),
        )
        .mount(&t.server)
        .await;
    mount_delayed_pages_alt(&t.server).await;

    let state_dir = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let base = t.server.uri();

    let crawl_args = |resume: bool| {
        let mut args = Vec::new();
        if resume {
            args.push("--resume".to_string());
        }
        args.extend([
            "--state-dir".to_string(),
            state_dir.path().display().to_string(),
            "--url".to_string(),
            base.clone(),
            "--output".to_string(),
            t.out.path().display().to_string(),
            "--max-pages".to_string(),
            MAX_PAGES.to_string(),
            "--quiet".to_string(),
        ]);
        args
    };

    // Attempt 1: checkpoint-mode crawl (no --resume, explicit --state-dir),
    // signalled after the hub + 3 deep pages were fetched.
    let child = spawn_webfang(&crawl_args(false), cache.path(), "signalled crawl");
    wait_for_page_fetches(&t.server, 4).await;
    send_signal(&child, "INT", "signalled crawl");
    let status = wait_exit(child, "signalled crawl");
    assert!(
        status.success(),
        "signalled crawl must exit 0 (cooperative cancellation), got {status:?}"
    );

    // The F-39 bound: the persisted frontier is capped at the run's OWN budget.
    let mut checkpoint_files = Vec::new();
    for entry in std::fs::read_dir(state_dir.path()).unwrap() {
        let p = entry.unwrap().path();
        if p.file_name().is_some_and(|n| {
            n.to_string_lossy().starts_with("crawl_checkpoint")
                && n.to_string_lossy().ends_with(".json")
        }) {
            checkpoint_files.push(p);
        }
    }
    assert!(
        !checkpoint_files.is_empty(),
        "SIGINT must persist a crawl checkpoint in the state dir"
    );
    for file in &checkpoint_files {
        // Format (BincodeCheckpoint::save): CRC32 checksum (4 bytes,
        // native endian) + JSON payload. Skip the checksum and parse the
        // payload directly — the prefix is binary, so no text read.
        let raw = std::fs::read(file).unwrap();
        let payload = &raw[4.min(raw.len())..];
        let checkpoint: serde_json::Value = serde_json::from_slice(payload).unwrap_or_else(|e| {
            panic!("checkpoint {file:?} must parse as JSON after the CRC32 prefix: {e}")
        });
        let queued = checkpoint["queued"].as_array().expect("queued array").len();
        assert!(
            queued <= MAX_PAGES,
            "F-39: persisted frontier ({queued}) must be bounded by the run's own \
             max_pages ({MAX_PAGES}) — the whole discovered set must never be persisted"
        );
    }

    // Attempt 2: resume must not drain a foreign frontier — every deep page is
    // fetched at most twice (once per run) and the resumed run stays bounded.
    let resumed = spawn_webfang(&crawl_args(true), cache.path(), "resumed crawl");
    let resume_status = wait_exit(resumed, "resumed crawl");
    assert!(
        resume_status.success(),
        "resumed crawl must exit 0, got {resume_status:?}"
    );

    let final_counts = fetches_by_path(&t.server).await;
    for i in 0..30 {
        let p = format!("/deep-{i}");
        let count = final_counts.get(&p).copied().unwrap_or(0);
        assert!(
            count <= 2,
            "F-39: {p} fetched {count} times — a replayed foreign frontier would \
             re-fetch pages across runs"
        );
    }
}

/// 30 delayed deep pages for the F-39 fixture (`/deep-{i}`).
async fn mount_delayed_pages_alt(server: &MockServer) {
    for i in 0..30 {
        let p = format!("/deep-{i}");
        Mock::given(method("GET"))
            .and(path(&p))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(article_body(&format!("Deep {i}")))
                    .set_delay(PAGE_DELAY),
            )
            .mount(server)
            .await;
    }
}
