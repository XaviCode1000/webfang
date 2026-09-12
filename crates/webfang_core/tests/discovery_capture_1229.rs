//! F-05 single-fetch capture proof (#1229, slice 2).
//!
//! Discovery must produce (url, content) and the consumer must reuse it —
//! one HTTP request per page. A six-node star fixture (seed + 5 leaves) at
//! `max_depth=1` is fetched exactly 6 times; the consumer phase then proves
//! no refetch by running with the mock server gone. A second test pins the
//! byte-cap trip: capture stops, and the consumer falls back to re-fetch
//! for uncaptured pages (correctness preserved, memory bounded).

use std::sync::Arc;

use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::application::crawler::InMemoryContentSink;
use webfang_core::application::progress_observer::NoopObserver;
use webfang_core::cli::scrape_flow::scrape_urls;
use webfang_core::cli::url_discovery::discover_urls_unified;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::persistence::PersistenceMode;
use webfang_core::domain::{CorrelationId, CrawlerConfig, ValidUrl};

use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Wiremock loopbacks are literal-IP URLs, which production rejects at the
/// SSRF entry guard — bypass entry + resolver exactly as the scrape-flow
/// robots test does.
const SSRF_BYPASS: [(&str, &str); 2] = [
    (
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    ),
    ("WEBFANG_DISABLE_SSRF_RESOLVER", "1"),
];

/// Filler prose so every fixture page clears the 50-char minimum-content
/// guard through the real `extract_content` path.
const FILLER: &str = "The harbor ledger records every tide that ever reached the stone quay, \
    and the clerks copy each entry twice so no storm can erase the account of what the sea returned.";

const LEAF_TITLE: &str = "Leaf page";

/// Build quiet discovery-shaped `CrawlOptions` pointing at `seed`.
fn star_opts(seed: &str) -> CrawlOptions {
    let url = ValidUrl::parse(seed).expect("wiremock seed must parse");
    let mut opts = CrawlOptions {
        url,
        ..Default::default()
    };
    opts.crawl.max_depth = 1;
    opts.crawl.max_pages = 10;
    opts.crawl.ignore_robots = true;
    opts.export.quiet = true;
    opts
}

/// Build the engine config the orchestrator discovery path uses.
fn star_config(seed: &url::Url) -> CrawlerConfig {
    CrawlerConfig::builder(seed.clone())
        .max_depth(1)
        .max_pages(10)
        .ignore_robots(true)
        .timeout_secs(5)
        .build()
}

/// Mount the six-node star: `/` links to `/p0..=p4`, leaves carry real prose.
///
/// With `strict`, the mocks pin exact request counts (the single-fetch
/// proof); without it they serve unboundedly so the re-fetch fallback can be
/// counted behaviorally via `received_requests`.
async fn mount_star(server: &MockServer, strict: bool) {
    let base = server.uri();
    let links: String = (0..5)
        .map(|i| format!(r#"<a href="{base}/p{i}">leaf {i}</a>"#))
        .collect();
    let mut seed_mock = Mock::given(method("GET")).and(path("/")).respond_with(
        ResponseTemplate::new(200).set_body_string(format!(
            "<html><head><title>Seed</title></head>\
             <body><h1>Seed</h1><p>{FILLER}</p><nav>{links}</nav><p>{FILLER}</p></body></html>"
        )),
    );
    if strict {
        seed_mock = seed_mock.expect(1);
    }
    seed_mock.mount(server).await;
    let mut leaf_mock = Mock::given(method("GET"))
        .and(path_regex("/p[0-4]"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<html><head><title>{LEAF_TITLE}</title></head>\
             <body><article><h1>{LEAF_TITLE}</h1><p>{FILLER}</p><p>{FILLER}</p></article></body></html>"
        )));
    if strict {
        leaf_mock = leaf_mock.expect(5);
    }
    leaf_mock.mount(server).await;
}

/// Single fetch per page (F-05, #1229 slice 2).
///
/// Unified discovery with a bounded sink yields 6 URLs and 6 captured bodies
/// over exactly 6 HTTP requests; the consumer then reuses the captured bodies
/// with the server dropped, so any refetch would fail loudly.
#[tokio::test]
async fn discovery_single_fetch_counts_requests() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    mount_star(&server, true).await;
    let base = server.uri();
    let seed_str = format!("{base}/");
    let seed_url = url::Url::parse(&seed_str).expect("seed must parse");

    let sink = Arc::new(InMemoryContentSink::new());
    let output = discover_urls_unified(
        star_config(&seed_url),
        &star_opts(&seed_str),
        &PersistenceMode::Disabled,
        Some(Arc::clone(&sink)),
    )
    .await
    .expect("star discovery must succeed");

    assert_eq!(
        output.urls.len(),
        6,
        "seed + 5 leaves must all be discovered"
    );
    assert_eq!(
        output.pages.len(),
        6,
        "every fetched page must be captured exactly once"
    );
    let seen = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        seen.len(),
        6,
        "discovery must issue one HTTP request per page, got {}",
        seen.len()
    );

    // The server is gone: cache hits need zero fetches, so any refetch fails.
    drop(server);
    let opts = star_opts(&seed_str);
    let (results, failures, _blocked) = scrape_urls(
        &output.urls,
        &ScraperConfig::default(),
        &opts,
        &NoopObserver,
        None,
        None,
        &CorrelationId::new(),
        &tokio_util::sync::CancellationToken::new(),
        &output.pages,
    )
    .await
    .expect("consumer setup must succeed");

    assert!(
        failures.is_empty(),
        "no refetch may fail with the server gone, got: {failures:?}"
    );
    assert_eq!(
        results.len(),
        6,
        "all six pages must come from discovery capture"
    );
}

/// The byte cap stops capture; the consumer re-fetches the rest (F-05, #1229).
///
/// With a tiny budget only the first pages fit: capture latches the cap
/// (the warn path), and the consumer still returns all six pages — hits
/// from capture, misses via normal re-fetch against the live server.
#[tokio::test]
async fn capture_stops_at_byte_cap() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    mount_star(&server, false).await;
    let base = server.uri();
    let seed_str = format!("{base}/");
    let seed_url = url::Url::parse(&seed_str).expect("seed must parse");

    // Budget fits the seed page (~750 B) but not a second page: the first
    // leaf trips the cap, so capture stops with a strict subset held.
    let sink = Arc::new(InMemoryContentSink::with_max_bytes(1024));
    let output = discover_urls_unified(
        star_config(&seed_url),
        &star_opts(&seed_str),
        &PersistenceMode::Disabled,
        Some(Arc::clone(&sink)),
    )
    .await
    .expect("capped discovery must succeed");

    assert_eq!(
        output.urls.len(),
        6,
        "the cap bounds memory, never discovery: all URLs still found"
    );
    assert!(
        output.pages.len() < 6,
        "capture must stop at the cap, got {} of 6 pages",
        output.pages.len()
    );
    assert!(
        !output.pages.is_empty(),
        "the seed must fit the budget so partial reuse is exercised"
    );
    assert!(
        sink.cap_exceeded(),
        "the warn-and-stop path must have tripped"
    );

    // Server stays alive: uncaptured pages fall back to a normal re-fetch.
    let opts = star_opts(&seed_str);
    let (results, failures, _blocked) = scrape_urls(
        &output.urls,
        &ScraperConfig::default(),
        &opts,
        &NoopObserver,
        None,
        None,
        &CorrelationId::new(),
        &tokio_util::sync::CancellationToken::new(),
        &output.pages,
    )
    .await
    .expect("consumer setup must succeed");

    // Exactly the uncaptured pages were re-fetched: discovery's 6 plus one
    // fetch per miss, nothing more.
    let seen = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        seen.len(),
        6 + (6 - output.pages.len()),
        "only uncaptured pages may be re-fetched, got {} requests",
        seen.len()
    );
    assert!(
        failures.is_empty(),
        "re-fetch fallback must preserve correctness, got: {failures:?}"
    );
    assert_eq!(
        results.len(),
        6,
        "captured hits plus re-fetched misses must cover every URL"
    );
}
