//! Integration tests: the sitemap-index aggregate stays under the configured
//! `memory_limit_mb` (issue #1822).
//!
//! The `MemoryManager` estimator charges 2 KB per URL against
//! `SitemapConfig::memory_limit_mb`. With a 1 MB budget the aggregate limit
//! sits at 525 URLs (525 x 2000 = 1,050,000 >= 1 MiB), while a single child
//! of at most 524 URLs passes the per-parse check — so each child below is
//! sized to trip ONLY the aggregate gate: three 200-URL children exceed the
//! budget together, two do not, and a standalone sitemap just under the
//! budget parses successfully.
//!
//! Observable behavior is asserted through the public parser port with
//! wiremock (ephemeral adapter, no real network); assertions match the typed
//! error, never message strings.

use webfang_core::domain::crawler_port::SitemapConfig;
use webfang_core::domain::CorrelationId;
use webfang_core::infrastructure::crawler::{SitemapError, SitemapParser};

// Ephemeral adapters: wiremock mocks the sitemap origin on loopback, and
// EnvGuard disarms the SSRF entry guard for that loopback mock (#1382/#1369).
use webfang_test_utils::EnvGuard;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// URLs per child sitemap: 200 x 2000 B = ~0.4 MB, under the 1 MB per-parse
/// gate (which only trips at 525+ URLs) but large enough that three children
/// together (600 URLs = 1,200,000 B = 1 MB estimated) exceed the budget.
const URLS_PER_CHILD: usize = 200;

/// Parser with the deliberately tiny 1 MB aggregate budget.
///
/// Depth and concurrency are set explicitly because the builder's `build()`
/// only falls back to defaults for the size fields — a builder that sets
/// nothing else yields `max_depth = 0` (immediate `MaxDepthExceeded`) and
/// `concurrency = 0`. Mirrors what production `build_sitemap_parser` does.
fn bounded_parser() -> SitemapParser {
    SitemapParser::with_config(
        SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(3)
            .concurrency(5)
            .memory_limit_mb(1)
            .build(),
    )
    .unwrap()
}

/// Build a `urlset` sitemap with `count` URLs unique to `child` so cross-child
/// dedup cannot collapse the aggregate count.
fn child_sitemap(child: usize, count: usize) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for i in 0..count {
        xml.push_str(&format!(
            "  <url><loc>https://example.com/child{child}/page{i}</loc></url>\n"
        ));
    }
    xml.push_str("</urlset>\n");
    xml
}

/// Build a `sitemapindex` pointing at `child_count` `/childN.xml` siblings of
/// the running mock server.
fn index_sitemap(base: &str, child_count: usize) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for child in 0..child_count {
        xml.push_str(&format!(
            "  <sitemap><loc>{base}/child{child}.xml</loc></sitemap>\n"
        ));
    }
    xml.push_str("</sitemapindex>\n");
    xml
}

/// Mount the index and its `child_count` children on the mock server.
async fn mount_index(server: &MockServer, child_count: usize) {
    Mock::given(method("GET"))
        .and(path("/index.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(index_sitemap(&server.uri(), child_count))
                .insert_header("content-type", "application/xml"),
        )
        .mount(server)
        .await;
    for child in 0..child_count {
        Mock::given(method("GET"))
            .and(path(format!("/child{child}.xml")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(child_sitemap(child, URLS_PER_CHILD))
                    .insert_header("content-type", "application/xml"),
            )
            .mount(server)
            .await;
    }
}

/// Three 200-URL children together exceed the 1 MB aggregate budget: the
/// index parse fails with the typed MemoryLimitExceeded error instead of
/// accumulating every child's URL list unbounded (#1822 criterion 4).
#[tokio::test]
async fn index_children_together_over_budget_fail_with_typed_error() {
    let _entry_off = EnvGuard::entry_guard_off();
    let server = MockServer::start().await;
    mount_index(&server, 3).await;

    let parser = bounded_parser();
    let url = format!("{}/index.xml", server.uri());
    let err = parser
        .parse_from_url(&url, &CorrelationId::new())
        .await
        .unwrap_err();

    // 600 URLs x 2000 B = 1,200,000 B -> estimated 1 MB >= the 1 MB budget.
    assert!(
        matches!(err, SitemapError::MemoryLimitExceeded(1)),
        "expected the typed aggregate memory-limit error, got: {err:?}"
    );
}

/// Two 200-URL children (400 URLs = 800,000 B estimated) stay under the same
/// 1 MB budget: the index parse succeeds and returns every child URL.
#[tokio::test]
async fn index_children_under_budget_still_succeed() {
    let _entry_off = EnvGuard::entry_guard_off();
    let server = MockServer::start().await;
    mount_index(&server, 2).await;

    let parser = bounded_parser();
    let url = format!("{}/index.xml", server.uri());
    let urls = parser
        .parse_from_url(&url, &CorrelationId::new())
        .await
        .unwrap();

    assert_eq!(
        urls.len(),
        2 * URLS_PER_CHILD,
        "all child URLs must survive"
    );
}

/// A standalone (non-index) sitemap just under the budget — 500 URLs =
/// 1,000,000 B estimated, below the 525-URL trip point — still parses.
#[tokio::test]
async fn single_sitemap_just_under_budget_still_succeeds() {
    let _entry_off = EnvGuard::entry_guard_off();
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(child_sitemap(0, 500))
                .insert_header("content-type", "application/xml"),
        )
        .mount(&server)
        .await;

    let parser = bounded_parser();
    let url = format!("{}/sitemap.xml", server.uri());
    let urls = parser
        .parse_from_url(&url, &CorrelationId::new())
        .await
        .unwrap();

    assert_eq!(
        urls.len(),
        500,
        "the full under-budget set must be returned"
    );
}
