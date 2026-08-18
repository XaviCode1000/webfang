//! Robots.txt enforcement from CLI args.

use crate::assert_snapshot_redacted;
use crate::cmd;
use crate::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Seed page HTML linking to `/private/page`, padded so its extractable text
/// clears the 50-char minimum-content guard (XC-2).
const PRIVATE_SEED_PAGE: &str = r#"<html><body><a href="/private/page">Private</a>
             <p>The seed page carries plenty of substantive server-rendered text so it comfortably clears the fifty character minimum content guard.</p></body></html>"#;

/// Article page body with enough substantive text to clear the 50-char
/// minimum-content guard (XC-2).
const ARTICLE_PAGE: &str = r#"<html><body><main><article><h1>Page</h1>
<p>This article body carries plenty of substantive server-rendered text so it comfortably clears the fifty character minimum content guard.</p>
</article></main></body></html>"#;

/// Mock the seed page at `/` with the given HTML body.
async fn mock_seed_page(t: &BehavioralTest, body: &str) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&t.server)
        .await;
}

/// Build a sitemap XML listing the given paths, mock it, and return the
/// sitemap URL.
async fn setup_sitemap(t: &BehavioralTest, paths: &[&str]) -> String {
    let base = t.server.uri();
    let sitemap_url = format!("{base}/sitemap.xml");
    let urls: Vec<String> = paths
        .iter()
        .map(|p| format!("        <url><loc>{base}{p}</loc></url>"))
        .collect();
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
{}
    </urlset>"#,
        urls.join("\n")
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;
    sitemap_url
}

/// Run the scraper binary against the mock server with sitemap discovery.
/// Returns the process output.
fn run_scraper(t: &BehavioralTest, sitemap_url: &str, extra_args: &[&str]) -> std::process::Output {
    let base = t.server.uri();
    let mut command = cmd();
    command
        .arg("--url")
        .arg(&base)
        .arg("--sitemap-url")
        .arg(sitemap_url)
        .arg("--use-sitemap");
    for arg in extra_args {
        command.arg(arg);
    }
    command
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary")
}

/// Count requests whose path starts with the given prefix in the wiremock
/// request log.
async fn count_requests_with_prefix(t: &BehavioralTest, path_prefix: &str) -> usize {
    let requests = t.server.received_requests().await.unwrap();
    requests
        .iter()
        .filter(|r| r.url.path().starts_with(path_prefix))
        .count()
}

/// Count requests to exactly the given path in the wiremock request log.
async fn count_requests_to_path(t: &BehavioralTest, path: &str) -> usize {
    let requests = t.server.received_requests().await.unwrap();
    requests.iter().filter(|r| r.url.path() == path).count()
}

/// A Disallow: /private/ in robots.txt prevents the crawler from
/// fetching /private/page; the seed page is still fetched.
#[tokio::test]
async fn robots_txt_disallow_prevents_fetching() {
    let t = BehavioralTest::new().await;

    // robots.txt disallows /private/
    crate::common::mock_robots(&t.server, "User-agent: *\nDisallow: /private/\n").await;

    // Seed page links to /private/page.
    mock_seed_page(&t, PRIVATE_SEED_PAGE).await;

    // The /private/page endpoint is not mocked; if the crawler respects
    // robots.txt it will never reach it and wiremock returns 404 by default.

    // Sitemap lists the seed and the disallowed URL so discovery succeeds
    // (default discovery is sitemap-based); robots.txt enforcement must still
    // block /private/page at scrape time.
    let sitemap_url = setup_sitemap(&t, &["/", "/private/page"]).await;
    let output = run_scraper(&t, &sitemap_url, &[]);

    assert!(
        output.status.success(),
        "expected success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let private_requests = count_requests_with_prefix(&t, "/private").await;
    let seed_requests = count_requests_to_path(&t, "/").await;

    assert_eq!(
        private_requests, 0,
        "expected 0 requests to /private (disallowed by robots.txt), got {private_requests}"
    );
    assert_eq!(
        seed_requests, 1,
        "expected 1 request to seed /, got {seed_requests}"
    );
}

/// `--ignore-robots` overrides robots.txt: a path that would otherwise be
/// blocked (Disallow: /private/) IS fetched when the flag is supplied (#542
/// coverage extension). Observes the wiremock request log, no real network.
#[tokio::test]
async fn ignore_robots_flag_allows_disallowed_fetch() {
    let t = BehavioralTest::new().await;

    crate::common::mock_robots(&t.server, "User-agent: *\nDisallow: /private/\n").await;

    mock_seed_page(&t, PRIVATE_SEED_PAGE).await;

    Mock::given(method("GET"))
        .and(path("/private/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><h1>Secret</h1><p>hidden content only reachable when robots are ignored.</p></body></html>",
        ))
        .mount(&t.server)
        .await;

    let sitemap_url = setup_sitemap(&t, &["/", "/private/page"]).await;
    let output = run_scraper(&t, &sitemap_url, &["--ignore-robots"]);

    assert!(
        output.status.success(),
        "expected success with --ignore-robots, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let private_requests = count_requests_with_prefix(&t, "/private").await;
    let seed_requests = count_requests_to_path(&t, "/").await;

    assert_eq!(
        private_requests, 1,
        "expected /private/page to be fetched when --ignore-robots is set (robots.txt is bypassed)"
    );
    assert!(
        seed_requests >= 1,
        "expected the seed / to be fetched, got {seed_requests}"
    );
}

/// A robots.txt with `Disallow: /` blocks EVERY discovered URL: the run must
/// exit 77 (`EXIT_FORBIDDEN`) with the Spanish `--ignore-robots` hint instead
/// of the misleading "no pages scraped" network error (#705).
///
/// `--quiet` keeps the snapshot deterministic: the robots-block `info!` tracing
/// line is filtered (quiet = warn+), so stderr carries exactly the user-facing
/// `Error:` line emitted by `CliExit::Forbidden`'s `Termination` impl.
#[tokio::test]
async fn all_urls_blocked_by_robots_exits_77() {
    let t = BehavioralTest::new().await;

    // Disallow everything — including the seed.
    crate::common::mock_robots(&t.server, "User-agent: *\nDisallow: /\n").await;

    // Seed page — mocked so a robots regression (fetching despite Disallow)
    // would succeed and change the exit code instead of hitting a 404.
    mock_seed_page(
        &t,
        r#"<html><body><h1>Seed</h1><p>blocked content</p></body></html>"#,
    )
    .await;

    // Sitemap lists the seed so discovery succeeds (default discovery is
    // sitemap-based); robots.txt enforcement then blocks it at scrape time.
    let sitemap_url = setup_sitemap(&t, &["/"]).await;
    let output = run_scraper(&t, &sitemap_url, &[]);

    // Semantic invariants (the contract under test), asserted explicitly so
    // they hold even if the snapshot below drifts.
    assert_eq!(
        output.status.code(),
        Some(77),
        "all-blocked run must exit 77 (EXIT_FORBIDDEN), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("bloqueadas por robots.txt"),
        "stderr must carry the Spanish robots message, got: {stderr}"
    );
    assert!(
        stderr.contains("--ignore-robots"),
        "stderr must hint --ignore-robots, got: {stderr}"
    );

    // Snapshot the full stderr for regression detection. Routed through the
    // crate-root helper so it lands in `tests/behavioral/snapshots/`; it
    // redacts the temp dir, timestamps, ports, and ANSI codes.
    assert_snapshot_redacted("all_urls_blocked_by_robots_stderr", t.out.path(), stderr);
}

/// A site without robots.txt (404) must trigger exactly ONE robots.txt fetch
/// for the whole multi-page crawl instead of one per page-check (#794).
///
/// Wire-level proof: wiremock's `received_requests` counts `/robots.txt` hits.
/// Before the fix, a 5-page crawl with 94 links on the seed produced 459
/// robots fetches; the fail-open decision was never cached. The pages must
/// still be scraped (fail-open preserved).
#[tokio::test]
async fn missing_robots_txt_is_fetched_once_per_crawl() {
    let t = BehavioralTest::new().await;

    // No /robots.txt mock mounted — wiremock answers 404 by default, exactly
    // the "site without robots.txt" shape of the issue (books.toscrape.com).

    // Five pages served and listed in the sitemap so the scrape batch checks
    // robots.txt for each of them.
    for page in 1..=5 {
        Mock::given(method("GET"))
            .and(path(format!("/p{page}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_PAGE))
            .mount(&t.server)
            .await;
    }

    let paths: Vec<&str> = vec!["/p1", "/p2", "/p3", "/p4", "/p5"];
    let sitemap_url = setup_sitemap(&t, &paths).await;

    // `--delay-ms 0` keeps the multi-page crawl fast; sitemap is the source
    // of truth (plan_urls returns discovered URLs verbatim).
    let output = run_scraper(&t, &sitemap_url, &["--delay-ms", "0", "--max-pages", "5"]);

    assert!(
        output.status.success(),
        "a site without robots.txt must crawl successfully (fail-open), stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let robots_requests = count_requests_to_path(&t, "/robots.txt").await;
    assert_eq!(
        robots_requests, 1,
        "robots.txt must be fetched exactly once and the 404 fail-open decision cached \
         for the whole crawl (#794), got {robots_requests}"
    );

    // Every sitemap page was actually scraped (fail-open behavior preserved).
    for page in 1..=5 {
        let hits = count_requests_to_path(&t, &format!("/p{page}")).await;
        assert_eq!(hits, 1, "page /p{page} must be fetched exactly once");
    }
}
