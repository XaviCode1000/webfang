//! Integration tests for EngineOptions and crawl_site_with_options.
//!
//! Tests use wiremock for deterministic HTTP mocking — no network required.
//!
//! Run with: `cargo test --test integration_engine_tests`

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

/// Test 1: a fully-completed engine crawl leaves no checkpoint residue (F-01).
#[tokio::test]
async fn test_engine_with_checkpoint_enabled() {
    // Entry-guard allowance (F-06 + F-32, #1217): the engine fetches a
    // wiremock loopback literal through the production router.
    let _guard = webfang_test_utils::EnvGuard::with(&[(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    )]);
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
    let options = EngineOptions {
        checkpoint_path: Some(checkpoint_dir.clone()),
        session_pool_enabled: false,
        ignore_robots: true,
        js_strategy: JsStrategy::Static,
        autoscale_enabled: false,
        // Inject the factory so the JS-strategy router path is built; without it
        // `ProductionPageFetcher` silently falls back to the static `fetch_url`.
        downloader_factory: Some(Arc::new(DefaultDownloaderFactory)),
        ..Default::default()
    };

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
    // Entry-guard allowance (F-06 + F-32, #1217): the engine fetches a
    // wiremock loopback literal through the production router.
    let _guard = webfang_test_utils::EnvGuard::with(&[(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    )]);
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
    std::fs::create_dir_all(&checkpoint_dir).unwrap();

    // Pre-create a checkpoint that marks the seed as visited but keeps
    // /page2.html pending in the (now budget-bounded) queue — exactly what a
    // mid-crawl save_checkpoint leaves behind (#1234).
    let seed_url = format!("{}/index.html", server.uri());
    let page2_url = format!("{}/page2.html", server.uri());
    let mut visited = std::collections::HashSet::new();
    visited.insert(seed_url.clone());
    let state = CrawlCheckpoint {
        visited,
        queued: vec![page2_url],
        pages_crawled: 1,
        banned_domains: Vec::new(),
        version: CURRENT_CHECKPOINT_VERSION,
    };

    let store = BincodeCheckpoint::new();
    // F-01 (a): the engine scopes checkpoints per seed, so the pre-created
    // state must live at the scoped path to be picked up for resume.
    let checkpoint_file = CheckpointPath::new(&checkpoint_dir).file_for_seed(&seed_url);
    store.save(&state, &checkpoint_file).unwrap();

    // Now crawl with the same checkpoint dir — engine should resume
    let config = test_config(&server.uri());
    let options = EngineOptions {
        checkpoint_path: Some(checkpoint_dir),
        session_pool_enabled: false,
        ignore_robots: true,
        js_strategy: JsStrategy::Static,
        autoscale_enabled: false,
        // Inject the factory so the JS-strategy router path is built; without it
        // `ProductionPageFetcher` silently falls back to the static `fetch_url`.
        downloader_factory: Some(Arc::new(DefaultDownloaderFactory)),
        ..Default::default()
    };

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
