//! F-13 deterministic crawl order (#1237, slice 3).
//!
//! Same input + same flags must yield the same output set in the same order,
//! always. The frontier pins this with a fixed `ahash` seed plus a total
//! [`PrioritizedUrl`](webfang_core::infrastructure::crawler::url_queue::PrioritizedUrl)
//! order (priority, then lexicographic URL): equal priorities pop the
//! lexicographically smallest URL first, so the drain sequence is a pure
//! function of the push set — never of task timing or process randomness.

use std::cmp::Ordering;

use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::url_discovery::discover_urls_unified;
use webfang_core::domain::budget::{BudgetOverrides, CrawlConcurrency};
use webfang_core::domain::persistence::PersistenceMode;
use webfang_core::domain::{CrawlerConfig, DiscoveredUrl, ValidUrl};
use webfang_core::infrastructure::crawler::url_queue::{PrioritizedUrl, UrlQueue};

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Wiremock loopbacks are literal-IP URLs, which production rejects at the
/// SSRF entry guard — bypass entry + resolver exactly as the scrape-flow
/// robots test does.
const SSRF_BYPASS: [(&str, &str); 2] = [
    (
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    ),
    (
        webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
        "1",
    ),
];

/// Filler prose so every fixture page clears the 50-char minimum-content
/// guard through the real `extract_content` path.
const FILLER: &str = "The harbor ledger records every tide that ever reached the stone quay, \
    and the clerks copy each entry twice so no storm can erase the account of what the sea returned.";

/// Build a depth-0 HTML URL under the example.com fixture host.
fn queue_url(path: &str) -> DiscoveredUrl {
    let url =
        url::Url::parse(&format!("https://example.com{path}")).expect("fixture path must parse");
    let parent = url::Url::parse("https://example.com/").expect("fixture parent must parse");
    DiscoveredUrl::html(url, 0, parent)
}

/// Same input pushed twice must drain in the same order, pinned to the
/// lexicographic tie-break (F-13, #1237).
///
/// Insertion order is deliberately scrambled so a priority-only heap (which
/// pops equal priorities in heap-layout order) cannot satisfy the
/// lexicographic assertion by accident.
#[tokio::test]
async fn frontier_same_input_same_order() {
    const PATHS: [&str; 8] = [
        "/zebra", "/apple", "/mango", "/cherry", "/banana", "/kiwi", "/plum", "/fig",
    ];

    async fn drain_in_order() -> Vec<String> {
        let queue = UrlQueue::new();
        for path in PATHS {
            assert!(
                queue.push(queue_url(path)).await,
                "distinct fixture URLs must all enqueue"
            );
        }
        queue
            .drain_all()
            .await
            .iter()
            .map(|discovered| discovered.url.to_string())
            .collect()
    }

    let first = drain_in_order().await;
    let second = drain_in_order().await;
    assert_eq!(
        first, second,
        "same pushes must drain in the same order every time"
    );

    let mut lexicographic = first.clone();
    lexicographic.sort();
    assert_eq!(
        first, lexicographic,
        "same-priority drain must follow the lexicographic tie-break: {first:?}"
    );
}

/// Equal priorities break ties by URL string; priority still dominates (F-13).
///
/// `BinaryHeap` is a max-heap, so the lexicographically smallest URL must
/// compare *greatest* to pop first.
#[test]
fn prioritized_tie_break_lexicographic() {
    let apple = PrioritizedUrl::new(queue_url("/apple"), 200);
    let zebra = PrioritizedUrl::new(queue_url("/zebra"), 200);

    assert!(
        apple > zebra,
        "equal priority must order lexicographically smallest first"
    );
    assert_eq!(
        apple.cmp(&PrioritizedUrl::new(queue_url("/apple"), 200)),
        Ordering::Equal,
        "identical priority and URL must compare equal"
    );

    let important = PrioritizedUrl::new(queue_url("/zebra"), 1000);
    assert!(
        important > apple,
        "higher priority must still dominate the URL tie-break"
    );
}

/// Build quiet discovery-shaped `CrawlOptions` pointing at `seed`.
fn manylinks_opts(seed: &str) -> CrawlOptions {
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

/// Build the engine config the orchestrator discovery path uses, pinned to a
/// single in-flight fetch.
///
/// Sequential crawling removes fetch-completion timing from the order entirely:
/// seed → deterministic queue drain → pending → fetch → collect, so the output
/// order is a pure function of the frontier order slice 3 pins. Production
/// concurrency is untouched; only this test serializes.
fn manylinks_config(seed: &url::Url) -> CrawlerConfig {
    let crawl = CrawlConcurrency::new(1).expect("concurrency 1 must be valid");
    CrawlerConfig::builder(seed.clone())
        .max_depth(1)
        .max_pages(10)
        .ignore_robots(true)
        .timeout_secs(5)
        .budget_overrides(BudgetOverrides {
            crawl: Some(crawl),
            ..Default::default()
        })
        .build()
}

/// Mount the manylinks star: `/` links to `/p0..=p4` in SCRAMBLED document
/// order, leaves carry real prose but no out-links.
async fn mount_manylinks(server: &MockServer) {
    let base = server.uri();
    // Scrambled on purpose: heap-layout order must never leak into output.
    let links: String = [3, 0, 4, 1, 2]
        .iter()
        .map(|i| format!(r#"<a href="{base}/p{i}">leaf {i}</a>"#))
        .collect();
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "<html><head><title>Seed</title></head>\
             <body><h1>Seed</h1><p>{FILLER}</p><nav>{links}</nav><p>{FILLER}</p></body></html>"
        )))
        .mount(server)
        .await;
    for i in 0..5 {
        Mock::given(method("GET"))
            .and(path(format!("/p{i}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><head><title>Leaf {i}</title></head>\
                 <body><article><h1>Leaf {i}</h1><p>{FILLER}</p><p>{FILLER}</p></article></body></html>"
            )))
            .mount(server)
            .await;
    }
}

async fn discover_once(seed_url: &url::Url, seed_str: &str) -> Vec<String> {
    discover_urls_unified(
        manylinks_config(seed_url),
        &manylinks_opts(seed_str),
        &PersistenceMode::Disabled,
        None,
    )
    .await
    .expect("manylinks discovery must succeed")
    .urls
    .iter()
    .map(url::Url::as_str)
    .map(str::to_owned)
    .collect()
}

/// Repeated unified discovery returns children in IDENTICAL order (F-13).
///
/// Not just set-equal: the seed plus the five leaves must come back in the
/// exact lexicographic sequence on every run, even though the seed document
/// links them in scrambled order.
#[tokio::test]
async fn discovery_repeat_same_order() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let server = MockServer::start().await;
    mount_manylinks(&server).await;
    let base = server.uri();
    let seed_str = format!("{base}/");
    let seed_url = url::Url::parse(&seed_str).expect("seed must parse");

    let first = discover_once(&seed_url, &seed_str).await;
    let second = discover_once(&seed_url, &seed_str).await;

    assert_eq!(
        first, second,
        "repeated discovery runs must return children in identical order"
    );

    let expected: Vec<String> = std::iter::once(seed_str.clone())
        .chain((0..5).map(|i| format!("{base}/p{i}")))
        .collect();
    assert_eq!(
        first, expected,
        "discovery order must be the lexicographic frontier order, got: {first:?}"
    );
}
