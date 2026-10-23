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
//!
//! Portability: every test that DELIVERS a signal is unix-only. Windows has no
//! POSIX signal delivery to another process — the `kill` on the runner is
//! MSYS's, and it never reaches the child as a console control event, so the
//! assertions measured the harness instead of the product (#1631). The product
//! DOES wire the Windows drain path (`tokio::signal::ctrl_c()` under
//! `cfg(not(unix))`), but reaching it from a test needs a console-control
//! trigger this crate does not have; that coverage belongs with XP-S-02
//! (#1608), not with a cfg here.

use crate::common::{
    count_page_fetches, send_signal, spawn_webfang, wait_exit, wait_for_page_fetches, PAGE_DELAY,
};
use crate::BehavioralTest;
use std::collections::BTreeMap;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

/// Per-path page fetch counts (robots/sitemap excluded).
#[cfg_attr(not(unix), allow(dead_code))]
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
#[cfg_attr(not(unix), allow(dead_code))]
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
/// cancellation, #509 semantics), (2) the cancelled export DRAINS instead of
/// running to completion (#2244 decision A): every item whose D3 sequence
/// finished before the token was observed is COMMITTED and persisted, while
/// pages that had not reached their export turn have NO record — nothing
/// half-written (S2: the in-flight item never persists partially), and (3) the
/// resumed run finishes every remaining page without re-fetching anything the
/// drain already committed.
///
/// Scope note: this case pins drain SAFETY, not the drain's reachability. That
/// the token actually reaches the export is pinned by
/// `crash_matrix_test::crash_during_cancel_drain_sigint_drain_kills_then_recovers`
/// (the crash point arms only in the `Cancelled` branch, so an unwired token
/// makes that row fail with a clean exit). Here the invariants are the weaker,
/// honest ones: every persisted record is COMMITTED (never partial, S2), every
/// drained record belongs to a page fetched in this run and is never
/// re-fetched, and no page is fetched more than once per run. Before #2244 the
/// export ignored the token and committed EVERY fetched page of the signalled
/// run, which is exactly what made this case red under the new wiring.
#[cfg_attr(not(unix), allow(dead_code))]
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
    let (status, stderr) = wait_exit(child, label);
    assert!(
        status.success(),
        "{label}: graceful shutdown must exit 0 (cooperative cancellation), got {status:?}; \
         child stderr: {stderr}"
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

    // State persisted: the record store exists, and it tells the drain truth
    // (#2244 decision A): every record it holds is COMMITTED — the drain
    // persists completed items, never partial ones — and at least the first
    // drained item made it (S2: lo completado se persiste, lo cancelado no).
    // Pages that had not reached their export turn have NO record yet; the
    // resume below re-drives them (SC-gate: only COMMITTED allows skip).
    // Records live under the `StoreFile.records` envelope, keyed by canonical
    // URL (full origin incl. port).
    let state_file = state_dir.path().join("127.0.0.1.json");
    assert!(
        state_file.exists(),
        "{label}: record store must persist across the signal"
    );
    let records: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_file).unwrap()).unwrap();
    let committed: Vec<String> = records["records"]
        .as_object()
        .expect("record envelope must be an object")
        .iter()
        .map(|(url, record)| {
            assert_eq!(
                record["status"], "COMMITTED",
                "{label}: {url} — the drain may only persist COMMITTED items, never partial ones"
            );
            url.clone()
        })
        .collect();
    assert!(
        !committed.is_empty(),
        "{label}: the drain must persist at least the item that completed before the token was observed"
    );
    // Membership, not cardinality: a record for a URL this run never fetched
    // would be a phantom commit, and `committed.len() <= fetched.len()` alone
    // cannot see it.
    let fetched_urls: Vec<String> = fetched_paths.iter().map(|p| format!("{base}{p}")).collect();
    for url in &committed {
        assert!(
            fetched_urls.contains(url),
            "{label}: {url} has a drained record but was never fetched in the signalled run"
        );
    }

    // Attempt 2: resume completes the rest without re-fetching anything.
    let resumed = spawn_run("resume run");
    let (resume_status, resume_stderr) = wait_exit(resumed, "resume run");
    assert!(
        resume_status.success(),
        "{label}: resume run must exit 0, got {resume_status:?}; child stderr: {resume_stderr}"
    );

    let final_counts = fetches_by_path(&t.server).await;
    for p in &pages {
        // `fetched_before` was sampled right after the signalled run exited,
        // so the two counters below are PER RUN, not the cross-run total: a
        // `count <= 2` on the total would accept a double fetch inside a
        // single run, which is the regression this invariant exists to catch.
        let run1 = fetched_before.get(p).copied().unwrap_or(0);
        let total = final_counts.get(p).copied().unwrap_or(0);
        let run2 = total.saturating_sub(run1);
        assert!(
            run1 <= 1,
            "{label}: {p} fetched {run1} times in the signalled run (max 1 per run)"
        );
        assert!(
            run2 <= 1,
            "{label}: {p} fetched {run2} times in the resume run (max 1 per run)"
        );
        // The drain commit blocks a re-fetch of the committed page; a page the
        // drain left without a record is driven again by the resume — once,
        // never a replay loop.
        if committed.contains(&format!("{base}{p}")) {
            assert_eq!(
                total, 1,
                "{label}: {p} was COMMITTED by the drain and must NOT be re-fetched (got {total})"
            );
        } else {
            assert!(
                total >= 1,
                "{label}: {p} must have been driven by some run (got {total})"
            );
        }
    }
    assert!(
        t.find_files("md").len() >= 8,
        "{label}: the resumed run completes the remaining pages"
    );
}

/// P8-5 — SIGINT mid-scrape shuts down gracefully and the state is resumable
/// with zero duplicate fetches (ADR-0016 §3 "After signal" row).
///
/// Unix-only: see the module header — the signal cannot be delivered to a
/// child process on Windows, so the run would assert on the harness, not on
/// the product (#1631).
#[cfg(unix)]
#[tokio::test]
async fn p85_sigint_shutdown_is_resumable() {
    p85_signal_case("INT", "SIGINT run").await;
}

/// P8-5 — SIGTERM behaves exactly like SIGINT: graceful drain, persisted
/// state, idempotent resume.
///
/// Unix-only (SIGTERM does not exist on Windows): the product registers only
/// `tokio::signal::ctrl_c()` there (`cli/shutdown.rs`), so a SIGTERM case can
/// never observe the drain semantics it asserts.
#[cfg(unix)]
#[tokio::test]
async fn p85_sigterm_shutdown_is_resumable() {
    p85_signal_case("TERM", "SIGTERM run").await;
}

/// XP-S-03 (#1608) — SIGHUP behaves exactly like SIGINT/SIGTERM: closing the
/// terminal no longer kills the run abruptly; it drains, persists state and
/// resumes idempotently. Unix-only (SIGHUP does not exist on Windows).
#[cfg(unix)]
#[tokio::test]
async fn p85_sighup_shutdown_is_resumable() {
    p85_signal_case("HUP", "SIGHUP run").await;
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
///
/// Unix-only: the SIGINT delivery is the same non-portable step as P8-5
/// (#1631), and the checkpoint frontier is only reachable through it.
#[cfg(unix)]
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
    let (status, stderr) = wait_exit(child, "signalled crawl");
    assert!(
        status.success(),
        "signalled crawl must exit 0 (cooperative cancellation), got {status:?}; \
         child stderr: {stderr}"
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
    let (resume_status, resume_stderr) = wait_exit(resumed, "resumed crawl");
    assert!(
        resume_status.success(),
        "resumed crawl must exit 0, got {resume_status:?}; child stderr: {resume_stderr}"
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
#[cfg_attr(not(unix), allow(dead_code))]
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
