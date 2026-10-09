//! Integration tests for EngineOptions and crawl_site_with_options.
//!
//! Tests use wiremock for deterministic HTTP mocking — no network required.
//!
//! Run with: `cargo test --test integration_engine_tests`

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use url::Url;
use webfang_core::application::crawler::engine::EngineOptions;
use webfang_core::domain::{CorrelationId, JsStrategy};
use webfang_core::infrastructure::downloader::fetch_router::DefaultDownloaderFactory;
use webfang_core::{
    crawl_site_with_options, BincodeCheckpoint, CheckpointPath, CheckpointStore, CrawlCheckpoint,
    CrawlerConfig, CURRENT_CHECKPOINT_VERSION,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper: build a minimal CrawlerConfig pointing at the mock server.
fn test_config(base_url: &str) -> CrawlerConfig {
    let seed = Url::parse(&format!("{base_url}/index.html")).expect("valid mock URL");
    CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(5)
        .delay_ms(1)
        .concurrency(std::num::NonZeroUsize::new(1).expect("1 is non-zero"))
        .timeout_secs(5)
        .build()
}

/// Entry-guard allowance (F-06 + F-32, #1217): the engine fetches a wiremock
/// loopback literal through the production router. Bind the returned guard for
/// the rest of the test — it restores the original env on drop.
fn entry_guard() -> webfang_test_utils::EnvGuard {
    webfang_test_utils::EnvGuard::with(&[(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    )])
}

/// Engine options for a checkpoint-scoped crawl: static JS strategy plus the
/// injected downloader factory — without it `ProductionPageFetcher` silently
/// falls back to the static `fetch_url`.
fn checkpoint_options(checkpoint_dir: PathBuf) -> EngineOptions {
    EngineOptions {
        checkpoint_path: Some(checkpoint_dir),
        session_pool_enabled: false,
        ignore_robots: true,
        js_strategy: JsStrategy::Static,
        autoscale_enabled: false,
        downloader_factory: Some(Arc::new(DefaultDownloaderFactory)),
        ..Default::default()
    }
}

/// Pre-create the scoped checkpoint a mid-crawl truncation leaves behind (#1234):
/// the seed already visited, `queued` still pending, one page crawled. F-01 (a):
/// the engine scopes checkpoints per seed, so the state must live at the scoped
/// path to be picked up for resume. Returns the store and that scoped path.
fn seed_resume_checkpoint(
    checkpoint_dir: &Path,
    seed_url: &str,
    queued: Vec<String>,
) -> (BincodeCheckpoint, PathBuf) {
    std::fs::create_dir_all(checkpoint_dir).unwrap();
    let mut visited = std::collections::HashSet::new();
    visited.insert(seed_url.to_string());
    let state = CrawlCheckpoint {
        visited,
        queued,
        pages_crawled: 1,
        banned_domains: Vec::new(),
        version: CURRENT_CHECKPOINT_VERSION,
    };
    let store = BincodeCheckpoint::new();
    let checkpoint_file = CheckpointPath::new(checkpoint_dir).file_for_seed(seed_url);
    store.save(&state, &checkpoint_file).unwrap();
    (store, checkpoint_file)
}

/// Test 1: a fully-completed engine crawl leaves no checkpoint residue (F-01).
#[tokio::test]
async fn test_engine_with_checkpoint_enabled() {
    let _guard = entry_guard();
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/index.html"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body><h1>Hello</h1></body></html>"),
        )
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let checkpoint_dir = tmp.path().join("checkpoints");

    let config = test_config(&server.uri());
    let options = checkpoint_options(checkpoint_dir.clone());

    let result = crawl_site_with_options(config, options, &CorrelationId::new()).await;
    assert!(result.is_ok(), "crawl should succeed: {:?}", result.err());

    let crawl_result = result.unwrap();
    assert!(
        crawl_result.total_pages >= 1,
        "should crawl at least 1 page"
    );

    // F-01 (b): the single-page crawl completes fully, so its checkpoint
    // is cleaned up — no stale state may leak into the next identical run.
    let scoped =
        CheckpointPath::new(&checkpoint_dir).file_for_seed(&format!("{}/index.html", server.uri()));
    assert!(scoped
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("crawl_checkpoint_")));
    assert!(
        !scoped.exists(),
        "completed crawl must delete its scoped checkpoint at {}",
        scoped.display()
    );
}

/// Test 2: Engine resumes from an existing checkpoint.
///
/// Pre-creates a checkpoint whose seed is already visited and whose queue
/// still holds `/page2.html`, then verifies the engine actually crawls the
/// pending URL instead of finishing with zero work.
#[tokio::test]
async fn test_engine_resume_from_checkpoint() {
    let _guard = entry_guard();
    let server = MockServer::start().await;

    // Seed page with a link to /page2.html
    Mock::given(method("GET"))
        .and(path("/index.html"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<html><body>
                    <a href="/page2.html">Page 2</a>
                </body></html>"#,
        ))
        .mount(&server)
        .await;

    // page2 returns content
    Mock::given(method("GET"))
        .and(path("/page2.html"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body><h1>Page 2</h1></body></html>"),
        )
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let checkpoint_dir = tmp.path().join("checkpoints");

    // Pre-create a checkpoint that marks the seed as visited but keeps
    // /page2.html pending in the (now budget-bounded) queue.
    let seed_url = format!("{}/index.html", server.uri());
    let page2_url = format!("{}/page2.html", server.uri());
    let (_store, checkpoint_file) =
        seed_resume_checkpoint(&checkpoint_dir, &seed_url, vec![page2_url]);

    // Now crawl with the same checkpoint dir — engine should resume
    let config = test_config(&server.uri());
    let options = checkpoint_options(checkpoint_dir);

    let result = crawl_site_with_options(config, options, &CorrelationId::new()).await;
    assert!(
        result.is_ok(),
        "resume crawl should succeed: {:?}",
        result.err()
    );

    let crawl_result = result.unwrap();
    assert!(
        crawl_result.total_pages >= 1,
        "resume must crawl the pending queue, not finish empty (crawled {})",
        crawl_result.total_pages
    );

    let requests = server.received_requests().await.unwrap_or_default();
    let requested_paths: Vec<String> = requests.iter().map(|r| r.url.path().to_string()).collect();
    assert!(
        requested_paths.iter().any(|p| *p == "/page2.html"),
        "pending /page2.html must be fetched on resume, got: {requested_paths:?}"
    );
    assert!(
        requested_paths.iter().all(|p| *p != "/index.html"),
        "visited seed must not be re-crawled on resume, got: {requested_paths:?}"
    );
    // F-01 (b): the resumed crawl drains its queue and completes, so the
    // scoped checkpoint is cleaned up afterwards.
    assert!(
        !checkpoint_file.exists(),
        "completed resume must delete its scoped checkpoint at {}",
        checkpoint_file.display()
    );
}

/// Test 3 (#1843): a resumed run continues the persisted `pages_crawled`.
///
/// Pre-creates a checkpoint that already carries `pages_crawled: 1` (a prior
/// run crawled the seed), with two pages still queued. The resumed run gets a
/// budget of 1, so it crawls exactly one queued page and ends truncated — the
/// close verdict is `Write`, not `Delete`, and the persisted counter must be
/// `N + M = 2`, never the new process's own `M = 1` (the pre-fix regression:
/// `restore_checkpoint_state` restored visited/queued/banned but reset the
/// counter to 0).
#[tokio::test]
async fn issue_1843_resume_continues_persisted_pages_crawled() {
    let _guard = entry_guard();
    let server = MockServer::start().await;

    // Both queued pages are plain 200s; the seed is never fetched (visited).
    for p in ["/p1.html", "/p2.html"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>page</body></html>"),
            )
            .mount(&server)
            .await;
    }

    let tmp = TempDir::new().unwrap();
    let checkpoint_dir = tmp.path().join("checkpoints");

    // A prior run crawled the seed (N = 1) and left two pages queued —
    // exactly what a mid-crawl truncation leaves behind.
    let seed_url = format!("{}/index.html", server.uri());
    let (store, checkpoint_file) = seed_resume_checkpoint(
        &checkpoint_dir,
        &seed_url,
        vec![
            format!("{}/p1.html", server.uri()),
            format!("{}/p2.html", server.uri()),
        ],
    );

    // Budget of 1 against 2 queued pages: the resume truncates deterministically
    // (one page crawled, one still queued), so the close verdict is `Write` and
    // the file survives for inspection.
    let seed = Url::parse(&seed_url).expect("valid mock URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(1)
        .delay_ms(1)
        .concurrency(std::num::NonZeroUsize::new(1).expect("1 is non-zero"))
        .timeout_secs(5)
        .ignore_robots(true)
        .build();
    let options = checkpoint_options(checkpoint_dir);

    let result = crawl_site_with_options(config, options, &CorrelationId::new()).await;
    assert!(
        result.is_ok(),
        "resume crawl should succeed: {:?}",
        result.err()
    );
    let crawl_result = result.unwrap();
    assert_eq!(
        crawl_result.total_pages, 1,
        "the resumed run must crawl exactly its own budget (M = 1)"
    );

    assert!(
        checkpoint_file.exists(),
        "a truncated resume must keep its checkpoint (Write verdict) at {}",
        checkpoint_file.display()
    );
    let persisted = store
        .load(&checkpoint_file)
        .expect("checkpoint must reload");
    assert_eq!(
        persisted.pages_crawled, 2,
        "counter must continue the persisted total (N + M = 2), not the resumed process's own count"
    );
    assert!(
        persisted
            .visited
            .iter()
            .any(|u| u.ends_with("/p1.html") || u.ends_with("/p2.html")),
        "the crawled queued page must be recorded as visited: {:?}",
        persisted.visited
    );
}
