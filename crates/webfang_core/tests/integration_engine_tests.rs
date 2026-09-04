//! Integration tests for EngineOptions and crawl_site_with_options.
//!
//! Tests use wiremock for deterministic HTTP mocking — no network required.
//!
//! Run with: `cargo test --test integration_engine_tests`

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

/// Test 1: Engine with checkpoint enabled creates a checkpoint file.
#[tokio::test]
async fn test_engine_with_checkpoint_enabled() {
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

    let result = crawl_site_with_options(config, options).await;
    assert!(result.is_ok(), "crawl should succeed: {:?}", result.err());

    let crawl_result = result.unwrap();
    assert!(
        crawl_result.total_pages >= 1,
        "should crawl at least 1 page"
    );

    // Checkpoint file should exist after crawl
    let checkpoint_file = checkpoint_dir.join("crawl_checkpoint.json");
    assert!(
        checkpoint_file.exists(),
        "checkpoint file should be created at {}",
        checkpoint_file.display()
    );
}

/// Test 2: Engine resumes from an existing checkpoint.
///
/// Pre-creates a checkpoint whose seed is already visited and whose queue
/// still holds `/page2.html`, then verifies the engine actually crawls the
/// pending URL instead of finishing with zero work.
#[tokio::test]
async fn test_engine_resume_from_checkpoint() {
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
    // /page2.html pending in the queue — exactly what a mid-crawl
    // save_checkpoint leaves behind.
    let seed_url = format!("{}/index.html", server.uri());
    let page2_url = format!("{}/page2.html", server.uri());
    let mut visited = std::collections::HashSet::new();
    visited.insert(seed_url);
    let state = CrawlCheckpoint {
        visited,
        queued: vec![page2_url],
        pages_crawled: 1,
        banned_domains: Vec::new(),
        version: 1,
    };

    let store = BincodeCheckpoint::new();
    let checkpoint_file = checkpoint_dir.join("crawl_checkpoint.json");
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

    let result = crawl_site_with_options(config, options).await;
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
}
