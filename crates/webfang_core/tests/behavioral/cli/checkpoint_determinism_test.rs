//! F-01 checkpoint-scope determinism (issue #1214).
//!
//! Two identical crawls with default flags share one state dir (the
//! operator's default cache) and must produce identical output sets.
//!
//! Regression: with checkpointing default-on, the first crawl writes
//! `crawl_checkpoint.json` into the shared default dir and the second
//! identical crawl resumes from it (seed already visited), so the two runs
//! discover different URL sets. The fix resolves the default
//! (`--resume` off, `--state-dir` absent) to `PersistenceMode::Disabled`,
//! so no checkpoint is written and both runs discover the same set.
//!
//! Run with: `cargo nextest run --test behavioral checkpoint_determinism`

use crate::BehavioralTest;
use std::collections::BTreeSet;
use tempfile::TempDir;
use url::Url;
use webfang_core::application::crawl_options::CrawlOptions;
use webfang_core::cli::url_discovery::discover_urls_recursive;
use webfang_core::CrawlerConfig;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// #1369: the unified plain-discovery branch now rides the guard-checked
/// fetch router (the options seam wires the downloader factory), so the
/// wiremock loopback literal needs the entry+resolver bypass exactly as
/// `discovery_capture_1229` does.
const SSRF_BYPASS: [(&str, &str); 2] = [
    ("WEBFANG_DISABLE_SSRF_ENTRY_GUARD", "1"),
    ("WEBFANG_DISABLE_SSRF_RESOLVER", "1"),
];

const SEED_HTML: &str = r#"<html><body><article><h1>Seed</h1><p>Seed page carries enough substantive text to clear the minimum content guard.</p><a href="/page-a">Page A</a><a href="/page-b">Page B</a></article></body></html>"#;
const PAGE_A_HTML: &str = r#"<html><body><article><h1>Page A</h1><p>Page A carries enough substantive text to clear the minimum content guard.</p></article></body></html>"#;
const PAGE_B_HTML: &str = r#"<html><body><article><h1>Page B</h1><p>Page B carries enough substantive text to clear the minimum content guard.</p></article></body></html>"#;

/// Mount a deterministic 3-page site: `/seed` links to `/page-a` and `/page-b`.
async fn mount_determinism_site(t: &BehavioralTest) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/seed"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_A_HTML))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_B_HTML))
        .mount(&t.server)
        .await;
}

/// Run one crawl with default flags against `seed`, resolving persistence
/// exactly like the orchestrator (`PersistenceMode` over the shared state
/// dir), and return the sorted discovered URL set.
async fn crawl_once(seed: Url, shared_state_dir: &std::path::Path) -> BTreeSet<String> {
    let mut opts = CrawlOptions {
        url: webfang_core::domain::ValidUrl::try_from_url(seed.clone())
            .expect("test seed URL is a valid http(s) URL"),
        ..CrawlOptions::default()
    };
    opts.crawl.max_depth = 1;
    opts.crawl.max_pages = 10;

    let crawler_config = CrawlerConfig::builder(seed)
        .max_depth(1)
        .max_pages(10)
        .delay_ms(1)
        .concurrency(std::num::NonZeroUsize::new(1).expect("1 is non-zero"))
        .timeout_secs(5)
        .ignore_robots(true)
        .build();

    let persistence_mode = opts.crawl.persistence_mode(shared_state_dir);
    let discovered = discover_urls_recursive(crawler_config, &opts, &persistence_mode)
        .await
        .expect("discovery must succeed");
    discovered.into_iter().map(|u| u.to_string()).collect()
}

/// No `crawl_checkpoint*` residue may remain in the shared dir after a
/// default-flags crawl (Disabled writes nothing; a completed opt-in run
/// cleans up after itself).
fn checkpoint_residue(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("crawl_checkpoint"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Two identical crawls sharing one state dir must discover identical URL sets.
#[tokio::test]
async fn identical_crawls_produce_identical_output_sets() {
    let _env = webfang_test_utils::EnvGuard::with(&SSRF_BYPASS);
    let t = BehavioralTest::new().await;
    mount_determinism_site(&t).await;

    // Both runs share one state dir — the operator's default cache in the
    // F-01 repro (two identical `webfang` invocations on one machine).
    let shared_state = TempDir::new().unwrap();
    let seed = Url::parse(&format!("{}/seed", t.server.uri())).expect("valid URL");

    let first = crawl_once(seed.clone(), shared_state.path()).await;
    let second = crawl_once(seed.clone(), shared_state.path()).await;

    assert!(
        first.len() >= 3,
        "first crawl must discover seed + both pages, got {first:?}"
    );
    assert_eq!(
        first, second,
        "F-01: two identical crawls must produce identical output sets"
    );
    assert!(
        checkpoint_residue(shared_state.path()).is_empty(),
        "default-flags crawls must leave no checkpoint residue, found {:?}",
        checkpoint_residue(shared_state.path())
    );
}
