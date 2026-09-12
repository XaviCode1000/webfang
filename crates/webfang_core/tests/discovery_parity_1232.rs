//! F-14 discovery parity guard (#1232, slice 1).
//!
//! Dry-run and the real DOM path must share one discovery function behind
//! both call sites, so a wiremock fixture with nested links at depth 2
//! yields the same URL set. The depth-2 child proves the legacy
//! single-fetch path (one fetch, depth silently ignored) is not used in
//! DOM mode. Ordering is intentionally NOT pinned here: crawl task
//! completion order is nondeterministic until slice-3 determinism lands,
//! so this guard compares sets.

use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::url_discovery::discover_urls_unified;
use webfang_core::domain::persistence::PersistenceMode;
use webfang_core::domain::CrawlerConfig;
use webfang_core::domain::ValidUrl;

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Wiremock loopbacks are literal-IP URLs, which production rejects at the
/// SSRF entry guard — bypass entry + resolver exactly as `discovery_capture_1229`
/// does (#1369: the plain unified branch now rides the guard-checked router).
const SSRF_BYPASS: [(&str, &str); 2] = [
    ("WEBFANG_DISABLE_SSRF_ENTRY_GUARD", "1"),
    ("WEBFANG_DISABLE_SSRF_RESOLVER", "1"),
];

/// Build quiet `CrawlOptions` pointing at `seed` for discovery tests.
fn discovery_opts(seed: &str) -> CrawlOptions {
    let url = ValidUrl::parse(seed).expect("wiremock seed must parse");
    let mut opts = CrawlOptions {
        url,
        ..Default::default()
    };
    opts.crawl.max_depth = 2;
    opts.crawl.max_pages = 10;
    opts.export.quiet = true;
    opts
}

/// Build the engine config the orchestrator discovery path uses.
fn discovery_config(seed: &url::Url) -> CrawlerConfig {
    CrawlerConfig::builder(seed.clone())
        .max_depth(2)
        .max_pages(10)
        .ignore_robots(true)
        .timeout_secs(5)
        .build()
}

/// Sort URLs for order-insensitive set comparison.
///
/// Crawl task completion order is nondeterministic until slice-3
/// determinism lands, so parity asserts on the sorted set, not the
/// discovery order.
fn sorted_urls(urls: &[url::Url]) -> Vec<url::Url> {
    let mut sorted: Vec<url::Url> = urls.to_vec();
    sorted.sort();
    sorted
}

/// Dry-run parity with real discovery (F-14, #1232 slice 1).
///
/// Both shapes call the unified discovery with `sink=None`; the URL sets
/// must match (compared as sorted vectors — completion order is
/// nondeterministic until slice-3 determinism lands) and the depth-2
/// child must be present.
#[tokio::test]
async fn dry_run_parity_with_real_discovery() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    let base = server.uri();

    // Seed "/" links to "/level1" (depth 1).
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<html><body><a href="{base}/level1">level1</a></body></html>"#
        )))
        .mount(&server)
        .await;
    // "/level1" links to "/level1/level2" (depth 2).
    Mock::given(method("GET"))
        .and(path("/level1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<html><body><a href="{base}/level1/level2">level2</a></body></html>"#
        )))
        .mount(&server)
        .await;
    // Depth-2 leaf.
    Mock::given(method("GET"))
        .and(path("/level1/level2"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>leaf</body></html>"))
        .mount(&server)
        .await;

    let seed_str = format!("{base}/");
    let seed_url = url::Url::parse(&seed_str).expect("seed must parse");

    // Dry-run shape: options built as `run_dry_run` does, then unified call.
    let dry_opts = discovery_opts(&seed_str);
    let dry_cfg = discovery_config(&seed_url);
    let dry_output = discover_urls_unified(dry_cfg, &dry_opts, &PersistenceMode::Disabled, None)
        .await
        .expect("dry-run shaped unified discovery must succeed");

    // Real DOM shape: options built as `prepare_phase` does, then unified call.
    let dom_opts = discovery_opts(&seed_str);
    let dom_cfg = discovery_config(&seed_url);
    let dom_output = discover_urls_unified(dom_cfg, &dom_opts, &PersistenceMode::Disabled, None)
        .await
        .expect("DOM shaped unified discovery must succeed");

    assert_eq!(
        sorted_urls(&dry_output.urls),
        sorted_urls(&dom_output.urls),
        "dry-run and DOM discovery must return the same URL set"
    );

    let rendered: Vec<String> = dry_output.urls.iter().map(|u| u.to_string()).collect();
    assert!(
        rendered.iter().any(|u| u.ends_with("/level1/level2")),
        "depth-2 child must be present (legacy single-fetch path only reaches depth 1): {rendered:?}"
    );
}
