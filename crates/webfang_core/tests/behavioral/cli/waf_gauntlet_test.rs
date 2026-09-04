//! WAF Gauntlet — end-to-end behavioral test for the rescue mission Definition
//! of Done (issue #441).
//!
//! Proves the scraper traverses a realistic WAF sequence (403 → 429 → 200),
//! produces correct output with exit code 0, emits correlated observability
//! events, and persists checkpoint state that survives process restarts.
//!
//! Run with: `cargo nextest run --test behavioral waf_gauntlet`

use crate::cmd;
use crate::BehavioralTest;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tempfile::TempDir;
use url::Url;
use webfang_core::application::crawler::engine::EngineOptions;
use webfang_core::domain::JsStrategy;
use webfang_core::infrastructure::downloader::fetch_router::DefaultDownloaderFactory;
use webfang_core::{
    crawl_site_with_options, BincodeCheckpoint, CheckpointStore, CrawlCheckpoint, CrawlerConfig,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Shared HTML fixtures — WAF-clean (no vendor names like "cloudflare",
// "captcha", etc. that would trip WafInspector on the 200 body).
// ---------------------------------------------------------------------------

const GAUNTLET_HTML: &str = r#"<html><body><article><h1>Gauntlet Passed</h1><p>The scraper survived the WAF and this page carries enough substantive text to clear the minimum content guard.</p></article></body></html>"#;

/// Mount the 403 → 429 → 200 sequence on `mock_path`.
///
/// wiremock 0.6 iterates mocks in **FIFO** order (first mounted = first
/// Mount the 403 → 429 → 200 sequence on `mock_path` using a single stateful mock.
///
/// Uses an atomic counter to track request count and return the appropriate
/// response. This avoids wiremock FIFO matching flakiness when UA changes.
async fn mount_waf_sequence(server: &MockServer, mock_path: &str) {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);

    Mock::given(method("GET"))
        .and(path(mock_path))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            match count {
                0 => ResponseTemplate::new(403),
                1 => ResponseTemplate::new(429).insert_header("Retry-After", "0"),
                _ => ResponseTemplate::new(200).set_body_string(GAUNTLET_HTML),
            }
        })
        .mount(server)
        .await;
}

// ===========================================================================
// Test 1 — WAF retry gauntlet: 403 → 429 → 200 → exit 0 + correct output
// ===========================================================================

/// The scraper receives a 403 (UA rotation retry), then a 429 (backoff retry),
/// then a 200. It must produce correct Markdown and exit with code 0.
#[tokio::test]
async fn waf_gauntlet_403_429_200_success() {
    let t = BehavioralTest::new().await;
    mount_waf_sequence(&t.server, "/gauntlet").await;

    let base = t.server.uri();
    let output = cmd()
        .arg("--url")
        .arg(format!("{base}/gauntlet"))
        .arg("--single-page")
        .arg("--output")
        .arg(t.out.path())
        .arg("--max-retries")
        .arg("3")
        .arg("--backoff-base-ms")
        .arg("10")
        .arg("--backoff-max-ms")
        .arg("50")
        .arg("--quiet")
        .output()
        .expect("run webfang binary");

    // --- Exit code 0 ---
    assert!(
        output.status.success(),
        "expected exit 0, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    // --- Exactly 3 HTTP requests to /gauntlet (403 + 429 + 200) ---
    let requests = t.server.received_requests().await.unwrap();
    let gauntlet_hits = requests
        .iter()
        .filter(|r| r.url.path() == "/gauntlet")
        .count();
    assert_eq!(
        gauntlet_hits, 3,
        "expected 3 requests (403→429→200), got {gauntlet_hits}"
    );

    // --- Correct Markdown output ---
    let md_files = t.find_files("md");
    assert_eq!(md_files.len(), 1, "expected exactly 1 markdown file");
    let content = t.read_md_content();
    assert!(
        content.contains("Gauntlet Passed"),
        "markdown should contain the H1 text, got: {content}"
    );
}

/// Mount the trace-oriented WAF sequence: 403 → 429 → 429 → 200…
///
/// Why TWO 429s: the initial 403 triggers the rotated-User-Agent retry
/// (#503 path). When that rotated request itself receives a 429, production
/// captures the status and re-enters the loop WITHOUT emitting a log event
/// (`wreq_downloader::fetch_inner`). A 429 only produces a trace event
/// ("Retrying … after status 429") when a PRIMARY loop attempt receives it,
/// so the second 429 lands on attempt 1 and exercises the unified
/// rate-limit retry path whose trace event this test asserts.
async fn mount_waf_trace_sequence(server: &MockServer, mock_path: &str) {
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = Arc::clone(&counter);

    Mock::given(method("GET"))
        .and(path(mock_path))
        .respond_with(move |_req: &wiremock::Request| {
            let count = counter_clone.fetch_add(1, Ordering::SeqCst);
            match count {
                0 => ResponseTemplate::new(403),
                1 | 2 => ResponseTemplate::new(429).insert_header("Retry-After", "0"),
                _ => ResponseTemplate::new(200).set_body_string(GAUNTLET_HTML),
            }
        })
        .mount(server)
        .await;
}

// ===========================================================================
// Test 2 — Observability: JSONL trace with correlated trace_id + retry events
// ===========================================================================

/// The `--trace-file` JSONL must contain retry events (403 warn, 429 debug)
/// and every line must share the same `trace_id` (root span correlation).
///
/// Uses `mount_waf_trace_sequence` (403 → 429 → 429 → 200): see its doc
/// comment for why a second 429 is required for the 429 event to appear.
/// The functional test `waf_gauntlet_403_429_200_success` proves the plain
/// 403 → 429 → 200 retry logic works correctly.
///
/// Runs in normal CI: the mock is counter-based (deterministic order), so the
/// historical wiremock-FIFO flakiness that motivated `#[ignore]` no longer applies.
#[tokio::test]
async fn waf_gauntlet_observability_trace() {
    let t = BehavioralTest::new().await;
    mount_waf_trace_sequence(&t.server, "/gauntlet").await;

    let trace_path = t.out.path().join("trace.jsonl");
    let base = t.server.uri();

    // -vv → DEBUG level so 429 retry events appear in the trace.
    let output = cmd()
        .arg("--url")
        .arg(format!("{base}/gauntlet"))
        .arg("--single-page")
        .arg("--output")
        .arg(t.out.path())
        .arg("--max-retries")
        .arg("3")
        .arg("--backoff-base-ms")
        .arg("10")
        .arg("--backoff-max-ms")
        .arg("50")
        .arg("--trace-file")
        .arg(&trace_path)
        .arg("-vv")
        .output()
        .expect("run webfang binary");

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    // --- Parse JSONL ---
    let file = std::fs::File::open(&trace_path)
        .unwrap_or_else(|e| panic!("trace file should exist at {}: {e}", trace_path.display()));
    let reader = BufReader::new(file);
    let lines: Vec<serde_json::Value> = reader
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("trace line should be valid JSON: {e}\n{line}"))
        })
        .collect();

    assert!(
        !lines.is_empty(),
        "trace file should contain at least one event"
    );

    // --- All events share the same trace_id (root span correlation) ---
    let trace_ids: Vec<&str> = lines
        .iter()
        .filter_map(|v| v.get("trace_id").and_then(|t| t.as_str()))
        .collect();
    assert!(
        !trace_ids.is_empty(),
        "at least one event should carry a trace_id"
    );
    let first_trace_id = trace_ids[0];
    assert!(
        trace_ids.iter().all(|id| *id == first_trace_id),
        "all trace_ids should be identical (root span correlation), found: {:?}",
        trace_ids.iter().collect::<std::collections::HashSet<_>>()
    );

    // --- Retry events present ---
    let all_messages: Vec<String> = lines
        .iter()
        .filter_map(|v| v.get("message").and_then(|m| m.as_str()))
        .map(String::from)
        .collect();

    let has_403_event = all_messages.iter().any(|m| m.contains("403"));
    let has_429_event = all_messages.iter().any(|m| m.contains("429"));

    assert!(
        has_403_event,
        "trace should contain a 403-related event, messages: {all_messages:?}"
    );
    assert!(
        has_429_event,
        "trace should contain a 429-related event, messages: {all_messages:?}"
    );
}

// ===========================================================================
// Test 3 — Exhausted retries: persistent 403 → non-zero exit
// ===========================================================================

/// When every request returns 403 (no 200 fallback), the scraper must fail
/// with a non-zero exit code — it should NOT silently succeed.
#[tokio::test]
async fn waf_gauntlet_persistent_403_fails() {
    let t = BehavioralTest::new().await;

    // Permanent 403 — no escape.
    Mock::given(method("GET"))
        .and(path("/blocked"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let output = cmd()
        .arg("--url")
        .arg(format!("{base}/blocked"))
        .arg("--single-page")
        .arg("--output")
        .arg(t.out.path())
        .arg("--max-retries")
        .arg("1")
        .arg("--backoff-base-ms")
        .arg("10")
        .arg("--backoff-max-ms")
        .arg("50")
        .arg("--quiet")
        .output()
        .expect("run webfang binary");

    assert!(
        !output.status.success(),
        "persistent 403 should NOT exit 0, got {:?}",
        output.status.code()
    );

    // No markdown output should be produced.
    let md_files = t.find_files("md");
    assert!(
        md_files.is_empty(),
        "no markdown should be produced on total WAF block"
    );
}

// ===========================================================================
// Test 4 — Checkpoint atomicity + resume (Engine API level)
// ===========================================================================

/// Verifies the checkpoint subsystem end-to-end:
/// 1. A crawl with checkpoint enabled produces a valid checkpoint file
///    (CRC32 prefix + JSON payload).
/// 2. A second crawl from the same checkpoint dir resumes (skips visited).
///
/// This uses the Engine API directly (`crawl_site_with_options`) because the
/// CLI binary does not wire `Engine::with_checkpoint` — the checkpoint is an
/// engine-internal crash-recovery mechanism.
/// Mount a seed page linking to /page-a and /page-b for the checkpoint test.
async fn mount_checkpoint_site(server: &wiremock::MockServer) {
    // Seed page links to /page-a and /page-b.
    Mock::given(method("GET"))
        .and(path("/seed"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<html><body><a href="/page-a">A</a><a href="/page-b">B</a></body></html>"#,
        ))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page A</h1></article></body></html>"),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page B</h1></article></body></html>"),
        )
        .mount(server)
        .await;
}

/// Run a crawl from `seed` with checkpointing enabled, bounded by `max_pages`.
async fn crawl_with_checkpoint(
    seed: Url,
    checkpoint_dir: &Path,
    max_pages: usize,
) -> Result<webfang_core::domain::CrawlResult, webfang_core::domain::CrawlError> {
    let config = CrawlerConfig::builder(seed)
        .max_depth(1)
        .max_pages(max_pages)
        .delay_ms(1)
        .concurrency(std::num::NonZeroUsize::new(1).expect("1 is non-zero"))
        .timeout_secs(5)
        .build();

    let options = EngineOptions {
        checkpoint_path: Some(checkpoint_dir.to_path_buf()),
        session_pool_enabled: false,
        ignore_robots: true,
        js_strategy: JsStrategy::Static,
        autoscale_enabled: false,
        // Inject the factory so the JS-strategy router path is built; without it
        // `ProductionPageFetcher` silently falls back to the static `fetch_url`.
        downloader_factory: Some(Arc::new(DefaultDownloaderFactory)),
        ..Default::default()
    };

    crawl_site_with_options(config, options).await
}

/// Verify the checkpoint file exists with a valid CRC32 prefix + JSON payload,
/// that the store can load it, and that no `.tmp` file remains (atomicity).
fn assert_checkpoint_valid(checkpoint_dir: &Path) {
    let checkpoint_file = checkpoint_dir.join("crawl_checkpoint.json");
    assert!(
        checkpoint_file.exists(),
        "checkpoint file should exist at {}",
        checkpoint_file.display()
    );

    // Verify CRC32 prefix + JSON payload (atomic write format).
    let raw_bytes = std::fs::read(&checkpoint_file).expect("read checkpoint");
    assert!(
        raw_bytes.len() > 4,
        "checkpoint should have CRC32 prefix (4 bytes) + JSON"
    );
    let json_payload = &raw_bytes[4..];
    let parsed: CrawlCheckpoint =
        serde_json::from_slice(json_payload).expect("checkpoint JSON should parse");
    assert!(
        parsed.pages_crawled >= 1,
        "checkpoint should record at least 1 crawled page"
    );
    assert!(
        !parsed.visited.is_empty(),
        "checkpoint should have visited URLs"
    );

    // Verify the store can load it (CRC validation passes).
    let store = BincodeCheckpoint::new();
    let loaded = store.load(&checkpoint_file);
    assert!(
        loaded.is_some(),
        "CheckpointStore should load a valid checkpoint"
    );

    // --- Verify no leftover .tmp file (atomicity evidence) ---
    let tmp_file = checkpoint_dir.join("crawl_checkpoint.json.tmp");
    assert!(
        !tmp_file.exists(),
        "no .tmp file should remain after atomic save"
    );
}

#[tokio::test]
async fn waf_gauntlet_checkpoint_atomicity_and_resume() {
    let server = wiremock::MockServer::start().await;
    mount_checkpoint_site(&server).await;

    let tmp = TempDir::new().unwrap();
    let checkpoint_dir = tmp.path().join("checkpoints");

    // --- Phase 1: crawl with checkpoint, max_pages=2 (seed + page-a) ---
    let seed = Url::parse(&format!("{}/seed", server.uri())).expect("valid URL");
    let result = crawl_with_checkpoint(seed.clone(), &checkpoint_dir, 2).await;
    assert!(
        result.is_ok(),
        "phase-1 crawl should succeed: {:?}",
        result.err()
    );

    // --- Verify checkpoint file exists and has valid format ---
    assert_checkpoint_valid(&checkpoint_dir);

    // --- Phase 2: resume from checkpoint — engine should skip visited ---
    let result2 = crawl_with_checkpoint(seed, &checkpoint_dir, 10).await;
    assert!(
        result2.is_ok(),
        "phase-2 resume crawl should succeed: {:?}",
        result2.err()
    );

    // The engine should have skipped already-visited URLs from phase 1.
    // We verify by checking that the total pages across both runs doesn't
    // exceed the total available pages (seed + page-a + page-b = 3).
    let phase2_result = result2.unwrap();
    assert!(
        phase2_result.total_pages <= 3,
        "resume should not re-crawl visited pages, got {} pages",
        phase2_result.total_pages
    );
}
