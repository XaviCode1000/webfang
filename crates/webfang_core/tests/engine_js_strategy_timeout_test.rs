//! Engine-level regression test for issue #280.
//!
//! `Engine::with_js_strategy()` previously built its `WreqDownloader` with a
//! hardcoded 30s timeout, ignoring `CrawlerConfig::timeout_secs`. This test
//! drives the public `crawl_site_with_options` API — the only entry point that
//! applies `with_js_strategy` — against a slow endpoint and asserts the crawl
//! aborts near the configured 2s timeout instead of hanging ~30s.
//!
//! No CLI path reaches `with_js_strategy` today (CLI crawl mode goes through
//! `scrape_flow`; batch/MCP use `crawl_site`, which leaves the fetch router
//! unset), so this API-level test is the only behavioral coverage of the fix.

use std::time::{Duration, Instant};

use tokio::time::timeout;
use url::Url;
use webfang_core::application::{crawl_site_with_options, EngineOptions};
use webfang_core::domain::{CrawlerConfig, JsStrategy};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// `crawl_site_with_options` with `JsStrategy::Static` must honor
/// `CrawlerConfig::timeout_secs`: a 30s-delayed response with a 2s configured
/// timeout must abort in well under 10s and yield no successfully crawled pages.
#[tokio::test]
async fn engine_js_strategy_respects_config_timeout() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(30)),
        )
        .mount(&server)
        .await;

    let seed: Url = format!("{}/slow", server.uri())
        .parse()
        .expect("valid seed URL");
    let config = CrawlerConfig::builder(seed)
        .max_depth(0)
        .max_pages(1)
        .timeout_secs(2)
        .delay_ms(1)
        .ignore_robots(true)
        .build();

    let options = EngineOptions {
        js_strategy: JsStrategy::Static,
        ignore_robots: true,
        ..Default::default()
    };

    let start = Instant::now();
    let result = timeout(
        Duration::from_secs(15),
        crawl_site_with_options(config, options),
    )
    .await
    .expect("crawl must not hang — with_js_strategy must honor config.timeout_secs");
    let elapsed = start.elapsed();

    // The engine must have actually attempted the seed fetch — guards against
    // a vacuous pass where the crawl aborts before any request is sent.
    let requests = server.received_requests().await.expect("requests recorded");
    assert!(
        requests.iter().any(|r| r.url.path() == "/slow"),
        "engine must attempt the seed URL via the JS strategy fetch router"
    );

    assert!(
        elapsed < Duration::from_secs(10),
        "engine with JS strategy should time out near 2s, took {elapsed:?}"
    );

    match result {
        Ok(crawl) => {
            assert_eq!(
                crawl.total_pages, 0,
                "timed-out seed must not produce crawled pages"
            );
            assert!(
                crawl.errors >= 1,
                "timed-out seed must be counted as an error"
            );
        },
        Err(e) => panic!("crawl should complete with errors, not fail outright: {e}"),
    }
}
