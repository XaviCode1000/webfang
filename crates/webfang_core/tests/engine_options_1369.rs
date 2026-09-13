//! #1369: the migrated batch and plain-discovery entries must honor the
//! explicit `EngineOptions` seam — no silent knobless defaults.
//!
//! Before the deprecation, `--batch` routed through `crawl_site` /
//! `crawl_site_capturing` and plain discovery through `crawl_site`, both of
//! which hard-wired the transport policy inside the engine. These tests pin
//! the observable contract of the migrated paths at the ports the callers
//! actually control:
//!
//! - BATCH: the content sink attached via `with_content_sink` must ride the
//!   per-task options down to the engine (#631 gotcha: without a sink the
//!   batch discards bodies, so every batch assertion runs with one). The
//!   robots value flows through the same option bag and is pinned at the
//!   builder seam — the seed-only batch contract never queries robots.
//! - DISCOVERY: the plain run (no sink, `PersistenceMode::Disabled`) now
//!   rides the same `build_discovery_engine_options` output as the capture /
//!   checkpoint branches, observable via the per-run robots behavior.
//!
//! Wiremock loopbacks are literal-IP URLs, which production rejects at the
//! SSRF entry guard — bypass entry + resolver exactly as
//! `discovery_capture_1229` does.

use std::sync::Arc;

use webfang_core::application::batch::{BatchJob, BatchProcessor};
use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::application::crawler::content_sink::CrawlContentSink;
use webfang_core::application::crawler::InMemoryContentSink;
use webfang_core::cli::url_discovery::discover_urls_unified;
use webfang_core::domain::persistence::PersistenceMode;
use webfang_core::domain::{CrawlerConfig, ValidUrl};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SSRF_BYPASS: [(&str, &str); 2] = [
    ("WEBFANG_DISABLE_SSRF_ENTRY_GUARD", "1"),
    ("WEBFANG_DISABLE_SSRF_RESOLVER", "1"),
];

/// Body long enough to clear the minimum-content guard, mirroring the
/// six-node fixture of `discovery_capture_1229`.
const FILLER: &str = "The harbor ledger records every tide that ever reached the stone quay, \
    and the clerks copy each entry twice so no storm can erase the account of what the sea returned.";

fn page_body() -> String {
    format!("<html><head><title>Page</title></head><body><p>{FILLER}</p></body></html>")
}

/// Seed-only batch config pointing at the wiremock root.
fn batch_config(seed: &url::Url, ignore_robots: bool) -> CrawlerConfig {
    CrawlerConfig::builder(seed.clone())
        .max_depth(0)
        .max_pages(1)
        .ignore_robots(ignore_robots)
        .timeout_secs(5)
        .build()
}

/// Mount `/` and a permissive `/robots.txt` so both robots modes are
/// observable through the request log (the Disallow-free body makes the page
/// crawlable when enforcement is on).
async fn mount_page_and_robots(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page_body()))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /\n"))
        .mount(server)
        .await;
}

async fn request_paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.url.path().to_string())
        .collect()
}

// ——— BATCH ———

/// The batch path must carry `ignore_robots` from the base config onto the
/// engine — and it must capture page bodies through the sink wired as an
/// `EngineOptions` field (#631 gotcha: without a sink there is nothing to
/// assert, so the sink is part of the pin).
#[tokio::test]
async fn batch_options_honor_ignore_robots_and_capture_sink() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    mount_page_and_robots(&server).await;
    let seed = url::Url::parse(&format!("{}/", server.uri())).expect("wiremock seed must parse");

    let sink = Arc::new(InMemoryContentSink::new());
    let processor = BatchProcessor::new(1)
        .expect("valid concurrency")
        .with_content_sink(Arc::clone(&sink) as Arc<dyn CrawlContentSink>);
    let job = BatchJob::new(
        "robots-ignored".to_string(),
        vec![seed.as_str().to_string()],
        batch_config(&seed, true),
    );

    let result = processor
        .process_batch(job)
        .await
        .expect("batch dispatch must succeed");
    assert_eq!(result.succeeded, 1, "the single batch URL must succeed");
    assert!(result.errors.is_empty(), "got: {:?}", result.errors);

    let paths = request_paths(&server).await;
    assert_eq!(
        paths.iter().filter(|p| *p == "/robots.txt").count(),
        0,
        "ignore_robots(true) on the batch config must reach the engine: {paths:?}"
    );
    let pages = sink.take_pages();
    assert_eq!(
        pages.len(),
        1,
        "the shared sink must receive every batch body"
    );
    assert!(
        pages[0].html.contains("harbor ledger"),
        "the captured body must be the fetched page"
    );
}

/// The run-wide options are built once and cloned per URL task — every task
/// must still share the one sink (an options-clone that dropped or deep-copied
/// the `Arc` sink would silently break the #631 batch export contract).
///
/// Note the robots *direction* is not observable through `--batch`: the
/// seed-only contract (#1215) fetches no child links, and the engine's robots
/// gate (`crawl_task`) filters link extraction, not the seed fetch. Both
/// directions of the propagated flag are pinned at the seam itself by the
/// `batch_engine_options_make_every_knob_source_explicit` unit test.
#[tokio::test]
async fn batch_shares_one_sink_across_every_url_task() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page_body()))
        .expect(2)
        .mount(&server)
        .await;
    let seed = url::Url::parse(&format!("{}/", server.uri())).expect("wiremock seed must parse");

    let sink = Arc::new(InMemoryContentSink::new());
    let processor = BatchProcessor::new(2)
        .expect("valid concurrency")
        .with_content_sink(Arc::clone(&sink) as Arc<dyn CrawlContentSink>);
    let job = BatchJob::new(
        "two-urls".to_string(),
        vec![seed.as_str().to_string(), seed.as_str().to_string()],
        batch_config(&seed, true),
    );

    let result = processor
        .process_batch(job)
        .await
        .expect("batch dispatch must succeed");
    assert_eq!(result.succeeded, 2, "both batch URLs must succeed");

    let pages = sink.take_pages();
    assert_eq!(
        pages.len(),
        2,
        "the shared sink must receive every task's body, got {}",
        pages.len()
    );
}

// ——— DISCOVERY (plain branch: no sink, no checkpoint) ———

/// Discovery-shaped quiet `CrawlOptions` for the six-node run.
fn plain_opts(seed: &str) -> CrawlOptions {
    let url = ValidUrl::parse(seed).expect("wiremock seed must parse");
    let mut opts = CrawlOptions {
        url,
        ..Default::default()
    };
    opts.crawl.max_depth = 1;
    opts.crawl.max_pages = 10;
    opts.export.quiet = true;
    opts
}

/// Star topology: `/` links to `/p0..=p4`; leaves carry no further links.
async fn mount_star(server: &MockServer) {
    let base = server.uri();
    let links: String = (0..5)
        .map(|i| format!(r#"<a href="{base}/p{i}">leaf {i}</a>"#))
        .collect();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<html><head><title>Seed</title></head><body><p>{FILLER}</p><nav>{links}</nav></body></html>"
        )))
        .mount(server)
        .await;
    for i in 0..5 {
        Mock::given(method("GET"))
            .and(path(format!("/p{i}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(page_body()))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /\n"))
        .mount(server)
        .await;
}

/// The plain discovery branch must build its options the same way the capture
/// branch does: with `ignore_robots(true)` on the config the whole run costs
/// exactly one request per page — zero robots.txt fetches.
#[tokio::test]
async fn discovery_plain_run_propagates_ignore_robots_option() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    mount_star(&server).await;
    let base = server.uri();
    let seed_str = format!("{base}/");
    let seed_url = url::Url::parse(&seed_str).expect("seed must parse");
    let config = CrawlerConfig::builder(seed_url)
        .max_depth(1)
        .max_pages(10)
        .ignore_robots(true)
        .timeout_secs(5)
        .build();

    let output = discover_urls_unified(
        config,
        &plain_opts(&seed_str),
        &PersistenceMode::Disabled,
        None,
    )
    .await
    .expect("plain discovery must succeed");

    assert_eq!(
        output.urls.len(),
        6,
        "seed + 5 leaves must all be discovered"
    );
    assert!(
        output.pages.is_empty(),
        "a sink-less discovery stays metadata-only"
    );
    let seen = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        seen.len(),
        6,
        "the propagated ignore_robots option must keep one request per page (no robots.txt)"
    );
}

/// Mirror test for the same branch: `ignore_robots(false)` must make the
/// plain run pay exactly one robots.txt fetch (per domain, not per page).
#[tokio::test]
async fn discovery_plain_run_enforces_robots_by_default() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    mount_star(&server).await;
    let base = server.uri();
    let seed_str = format!("{base}/");
    let seed_url = url::Url::parse(&seed_str).expect("seed must parse");
    let config = CrawlerConfig::builder(seed_url)
        .max_depth(1)
        .max_pages(10)
        .ignore_robots(false)
        .timeout_secs(5)
        .build();

    let output = discover_urls_unified(
        config,
        &plain_opts(&seed_str),
        &PersistenceMode::Disabled,
        None,
    )
    .await
    .expect("plain discovery must succeed");

    assert_eq!(output.urls.len(), 6, "enforcement must not block the run");
    let seen = server.received_requests().await.unwrap_or_default();
    let robots = seen
        .iter()
        .filter(|r| r.url.path() == "/robots.txt")
        .count();
    assert_eq!(
        robots, 1,
        "robots.txt is fetched exactly once per domain, got {robots}"
    );
}

/// #1369 hardening pin: plain discovery now pays the same guard chain the
/// capture path always had. The knobless else-branch is gone, so the plain
/// branch rides the factory-wired options and every fetch runs through the
/// SSRF entry guard plus the validating resolver (AGENTS.md fetch-guard
/// order). No `EnvGuard` here: both bypass envs stay at their production
/// default (unset → guards armed). With a loopback wiremock seed the run must
/// still COMPLETE (the engine's robots gate receives `PolicyRefused` and skips
/// the URL — Ok with zero URLs, not Err), and because the rejection is
/// pre-socket the mock server must observe zero requests of any kind. Seeds
/// that used to crawl on the knobless path are refused by design (#1355,
/// #1251); the operator hatch stays the documented
/// `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` / `WEBFANG_DISABLE_SSRF_RESOLVER` pair.
#[tokio::test]
async fn discovery_plain_run_rejects_loopback_seed_with_ssrf_guard_on() {
    let server = MockServer::start().await;
    mount_page_and_robots(&server).await;
    let base = server.uri();
    let seed_str = format!("{base}/");
    let seed_url = url::Url::parse(&seed_str).expect("seed must parse");
    let config = CrawlerConfig::builder(seed_url)
        .max_depth(1)
        .max_pages(10)
        .ignore_robots(false)
        .timeout_secs(5)
        .build();

    let output = discover_urls_unified(
        config,
        &plain_opts(&seed_str),
        &PersistenceMode::Disabled,
        None,
    )
    .await
    .expect("the armed guard cuts the seed pre-socket, so the run completes Ok");

    assert!(
        output.urls.is_empty(),
        "loopback seed with the guard chain on must discover zero URLs, got {:?}",
        output.urls
    );
    assert!(
        output.pages.is_empty(),
        "a refused run must not carry captured pages"
    );
    let seen = server.received_requests().await.unwrap_or_default();
    assert!(
        seen.is_empty(),
        "rejection is pre-socket: the mock must receive zero requests, got {seen:?}"
    );
}
