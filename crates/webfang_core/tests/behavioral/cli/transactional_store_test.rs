//! Transactional record store (#1230 / F-07) and honest corrupt-state reporting (P8-3).
//!
//! Both tests drive the REAL `webfang` binary through a shared `--state-dir`,
//! because the defect is cross-process by nature: `StoreLock` is `flock(2)`
//! (`infrastructure/export/record_store.rs`) and ADR-0014 records that loom
//! cannot model it. An in-process (thread-based) test would pass against the
//! broken code, so spawning the binary is the only harness that can falsify the
//! claim at process level.
//!
//! Ground truth is the mock server's own request log: every URL that was fetched
//! and exported MUST be present in the persisted record set. That is the
//! invariant F-07 broke (90 records on disk vs 102 URLs actually scraped).
//!
//! See the ignore-reason attribute on the concurrency test: measured against the
//! unfixed code, this process-level shape does NOT reproduce the loss, so the
//! deterministic falsifier lives in `tests/record_store_transaction_test.rs`.

use crate::BehavioralTest;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Host (no port) that `domain_from_url` derives from the wiremock origin — the
/// record store file is named `<domain>.json`.
const DOMAIN: &str = "127.0.0.1";

/// Pages per writer. Enough records that each export phase lasts long enough to
/// still be writing while another writer is starting.
const PAGES_PER_SET: usize = 20;

/// Concurrent writers. F-07 was measured with six, and the number matters: the
/// clobber needs every writer to reach `CommitSession::open()` before any of
/// them performs its first save.
const CONCURRENT_WRITERS: usize = 6;

/// How long the shared gate page blocks, so the export phases of the writers
/// start closer together than raw process-boot drift would allow.
const GATE_DELAY: Duration = Duration::from_millis(3000);

/// Per-page response delay. Kept small on purpose: the crawl only has to be long
/// enough for the gate to dominate it.
const PAGE_DELAY: Duration = Duration::from_millis(5);

/// Body that clears the minimum-content guard (same shape as `resume_test.rs`).
fn page_body(set: &str, i: usize) -> String {
    format!(
        "<html><body><article><h1>Page {set} {i}</h1><p>Body of page {set} {i} carries enough substantive text to pass the minimum-content guard without trouble.</p></article></body></html>"
    )
}

/// Mount `/{set}/p0 .. /{set}/p{n-1}`, each with a response delay, and return
/// their absolute URLs in sitemap order.
async fn mount_page_set(server: &MockServer, set: &str, n: usize) -> Vec<String> {
    let base = server.uri();
    let mut urls = Vec::with_capacity(n);
    for i in 0..n {
        let rel = format!("/{set}/p{i}");
        urls.push(format!("{base}{rel}"));
        Mock::given(method("GET"))
            .and(path(&rel))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(page_body(set, i))
                    .set_delay(PAGE_DELAY),
            )
            .mount(server)
            .await;
    }
    urls
}

/// Mount the shared slow page that nudges the writers' crawl phases together.
async fn mount_gate(server: &MockServer) -> String {
    let url = format!("{}/gate", server.uri());
    Mock::given(method("GET"))
        .and(path("/gate"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(page_body("gate", 0))
                .set_delay(GATE_DELAY),
        )
        .mount(server)
        .await;
    url
}

/// Mount a sitemap listing exactly `urls`, and return its own URL.
async fn mount_sitemap_for(server: &MockServer, name: &str, urls: &[String]) -> String {
    let base = server.uri();
    let sitemap_url = format!("{base}/{name}.xml");
    let items = urls
        .iter()
        .map(|u| format!("    <url><loc>{u}</loc></url>"))
        .collect::<Vec<_>>()
        .join("\n");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
{items}
</urlset>"#
    );
    crate::common::mock_sitemap(server, &sitemap_url, &xml).await;
    sitemap_url
}

/// Absolute path of the record store file inside `state_dir`.
fn state_file(state_dir: &Path) -> PathBuf {
    state_dir.join(format!("{DOMAIN}.json"))
}

/// Every URL present in the persisted record set (the `url` field of each
/// record, not the map key — the key is the canonical form).
///
/// Panics with the file path when the state file is missing or unparsable, so a
/// lost-update failure can never be mistaken for a parse failure.
fn persisted_urls(state_dir: &Path) -> BTreeSet<String> {
    let path = state_file(state_dir);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("record store {} unreadable: {e}", path.display()));
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("record store {} unparsable: {e}", path.display()));
    let records = value
        .get("records")
        .and_then(serde_json::Value::as_object)
        .unwrap_or_else(|| panic!("record store {} has no `records` object", path.display()));
    records
        .values()
        .map(|r| {
            r.get("url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("record without a `url` field in {}", path.display()))
                .to_string()
        })
        .collect()
}

/// Distinct page URLs the mock server actually served (sitemaps, gate and robots
/// excluded) — the work that really happened on the wire.
async fn scraped_urls(server: &MockServer) -> BTreeSet<String> {
    let base = server.uri();
    server
        .received_requests()
        .await
        .expect("read received requests")
        .iter()
        .map(|r| r.url.path().to_string())
        .filter(|p| p.starts_with("/w"))
        .map(|p| format!("{base}{p}"))
        .collect()
}

/// Environment shared with the behavioral harness: `WEBFANG_*` scrubbed, the SSRF
/// entry guard disarmed for the `127.0.0.1` mock, and a private `XDG_CACHE_HOME`
/// so a spawned run cannot touch the real cache.
fn writer_command(cache_dir: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(crate::common::webfang_path());
    for key in std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
    {
        command.env_remove(key);
    }
    command.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    command.env("XDG_CACHE_HOME", cache_dir);
    command
}

/// Spawn one `--resume` writer against the shared `--state-dir`.
///
/// `--no-checkpoint` keeps the engine frontier out of the measurement — the
/// record store is the subject. Each writer gets its own `--output` because two
/// processes sharing an export directory would race on files, which is a
/// different defect. `--delay-ms 0` removes the 1000 ms default politeness
/// delay, which alone would serialise the whole run.
fn spawn_writer(
    server: &MockServer,
    state_dir: &Path,
    out_dir: &Path,
    cache_dir: &Path,
    sitemap_url: &str,
) -> Child {
    let mut command = writer_command(cache_dir);
    command
        .arg("--resume")
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--url")
        .arg(server.uri())
        .arg("--output")
        .arg(out_dir)
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(sitemap_url)
        .arg("--max-pages")
        .arg((PAGES_PER_SET + 1).to_string())
        .arg("--delay-ms")
        .arg("0")
        .arg("--no-checkpoint")
        .arg("--quiet");
    command
        .spawn()
        .unwrap_or_else(|e| panic!("spawn webfang for {sitemap_url}: {e}"))
}

/// #1230 / F-07 — six concurrent `--resume` processes sharing one `--state-dir`
/// must persist the union of what they scraped, not the last writer's snapshot.
///
/// Each writer scrapes a **disjoint** page set: if two scraped the same URLs
/// their record sets would be identical and a whole-file clobber would be
/// unobservable. Disjointness is what turns "last writer wins" into data loss.
///
/// Evidence status (ADR-0016 §5, issue #1292): this is a DOCUMENTED STRESS
/// CHECK, not the F-07 evidence — the named evidence is the deterministic
/// `f07_*` tests in `tests/record_store_transaction_test.rs`. Read the ignore
/// reason before citing this as proof of anything.
#[tokio::test]
#[ignore = "asserts the no-loss invariant under real multi-process contention, but \
             measured 8/8 runs against the UNFIXED code with zero records lost: the \
             clobber needs every writer to reach CommitSession::open() before any \
             first save, a microsecond-wide window a fixture cannot force (wiremock \
             delays stagger, they never synchronise). So this is NOT a falsifier of \
             #1230 and must not gate CI. The deterministic falsifier is \
             tests/record_store_transaction_test.rs. Kept as an ignored stress check \
             that a future change does not regress concurrency: cargo test \
             -p webfang_core --test behavioral concurrent_resume_processes -- --ignored \
             --nocapture"]
async fn concurrent_resume_processes_lose_no_records() {
    let t = BehavioralTest::new().await;
    let gate = mount_gate(&t.server).await;

    let state_dir = TempDir::new().unwrap();
    let mut writers: Vec<Child> = Vec::new();
    // The temp dirs own the writers' output and cache; keep them alive until
    // every child has been reaped.
    let mut keep_alive: Vec<TempDir> = Vec::new();

    for i in 0..CONCURRENT_WRITERS {
        let set = format!("w{i}");
        let mut urls = mount_page_set(&t.server, &set, PAGES_PER_SET).await;
        urls.insert(0, gate.clone());
        let sitemap = mount_sitemap_for(&t.server, &format!("sitemap-{set}"), &urls).await;
        let out = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        writers.push(spawn_writer(
            &t.server,
            state_dir.path(),
            out.path(),
            cache.path(),
            &sitemap,
        ));
        keep_alive.push(out);
        keep_alive.push(cache);
    }

    let statuses: Vec<_> = writers
        .into_iter()
        .map(|mut w| w.wait().expect("wait writer"))
        .collect();
    for (i, status) in statuses.iter().enumerate() {
        // F-07's silence: every process exited 0 while losing state. Pinning the
        // exit codes means a regression cannot hide behind a failure.
        assert!(status.success(), "writer {i} should succeed: {status:?}");
    }

    let scraped = scraped_urls(&t.server).await;
    let persisted = persisted_urls(state_dir.path());
    let missing: BTreeSet<&String> = scraped
        .iter()
        .filter(|url| !persisted.contains(*url))
        .collect();

    assert!(
        missing.is_empty(),
        "concurrent --resume lost persisted state (#1230): {} of {} scraped URLs are \
         absent from the record store.\n  scraped: {}\n  persisted: {}\n  first 8 \
         missing: {:?}",
        missing.len(),
        scraped.len(),
        scraped.len(),
        persisted.len(),
        missing.iter().take(8).collect::<Vec<_>>(),
    );
}

/// P8-3 — an unreadable state file must be announced to the user in Spanish,
/// naming the file. Before this fix the only signal was an English
/// `tracing::WARN`, and AGENTS.md reserves English for internal logs: user-facing
/// text must be Spanish. A warning that exists only as a log line is how a user
/// ends up watching the tool silently discard their persisted state.
///
/// The run must still proceed — `resume_test::corrupt_state_falls_back_to_full_scrape`
/// pins the fallback. This test pins only the *explicitness* of it.
#[tokio::test]
async fn corrupt_state_file_is_reported_to_the_user() {
    let t = BehavioralTest::new().await;
    let alpha = mount_page_set(&t.server, "alpha", 2).await;
    let alpha_sitemap = mount_sitemap_for(&t.server, "sitemap-alpha", &alpha).await;

    let state_dir = TempDir::new().unwrap();
    let corrupt = state_file(state_dir.path());
    std::fs::write(&corrupt, "not valid json!!!").expect("write corrupt state");

    let out = TempDir::new().unwrap();
    let cache = TempDir::new().unwrap();
    let mut command = writer_command(cache.path());
    let output = command
        .arg("--resume")
        .arg("--state-dir")
        .arg(state_dir.path())
        .arg("--url")
        .arg(t.server.uri())
        .arg("--output")
        .arg(out.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&alpha_sitemap)
        .arg("--delay-ms")
        .arg("0")
        .arg("--no-checkpoint")
        .output()
        .expect("run binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "corrupt state must not panic: {stderr}"
    );
    // `Advertencia` is Spanish-only: the English `tracing::WARN` that exists
    // today cannot satisfy it, which is exactly the defect (P8-3).
    assert!(
        stderr.contains("Advertencia"),
        "corrupt state must be reported to the user in Spanish, not only as an \
         internal English log line; got stderr:\n{stderr}"
    );
    assert!(
        stderr.contains(&corrupt.display().to_string()),
        "the corrupt-state message must name the offending path; got stderr:\n{stderr}"
    );
}
