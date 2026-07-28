//! Crawl behavior: depth limits, page caps, include/exclude patterns.

use crate::cmd;
use crate::BehavioralTest;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// --max-depth 0 with --use-sitemap only scrapes the seed URL;
/// sitemap-discovered URLs are skipped.
#[tokio::test]
async fn max_depth_zero_only_scrapes_seed() {
    let t = BehavioralTest::new().await;

    // Seed page links to /page-a and /page-b as DOM-discovery fallback.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><a href=\"/page-a\">Page A</a><a href=\"/page-b\">Page B</a></body></html>",
        ))
        .mount(&t.server)
        .await;

    // /page-a and /page-b are listed in the sitemap but must not be fetched
    // when max-depth is 0.
    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page A</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page B</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let sitemap_url = format!("{}/sitemap.xml", base);
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{}/</loc></url>
    <url><loc>{}/page-a</loc></url>
</urlset>"#,
        base, base
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;

    let output = cmd()
        .arg("--url")
        .arg(base)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--max-depth")
        .arg("0")
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert!(
        output.status.success(),
        "expected success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = t.server.received_requests().await.unwrap();
    let seed_requests = requests.iter().filter(|r| r.url.path() == "/").count();
    let page_a_requests = requests
        .iter()
        .filter(|r| r.url.path() == "/page-a")
        .count();

    assert_eq!(
        seed_requests, 1,
        "expected 1 request to seed /, got {}",
        seed_requests
    );
    assert_eq!(
        page_a_requests, 0,
        "expected 0 requests to /page-a with max-depth 0, got {}",
        page_a_requests
    );

    let md_files = t.find_files("md");
    assert_eq!(
        md_files.len(),
        1,
        "expected 1 .md file, got {}",
        md_files.len()
    );
}

/// --max-pages 2 caps the crawl output to at most 2 .md files.
#[tokio::test]
async fn max_pages_limits_crawl_output() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><a href=\"/page-a\">A</a><a href=\"/page-b\">B</a><a href=\"/page-c\">C</a><a href=\"/page-d\">D</a><a href=\"/page-e\">E</a></body></html>",
        ))
        .mount(&t.server)
        .await;

    // Discovery order is non-deterministic (sitemap URLs pass through the crawl
    // budget optimizer), so --max-pages 2 may pick any two of the six URLs.
    // Mock all of them so whichever two are scraped succeed; the `<= 2` .md
    // assertion below is what actually validates the max-pages cap.
    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page A</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page B</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-c"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page C</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-d"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page D</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-e"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page E</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let sitemap_url = format!("{}/sitemap.xml", base);
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{}/</loc></url>
    <url><loc>{}/page-a</loc></url>
    <url><loc>{}/page-b</loc></url>
    <url><loc>{}/page-c</loc></url>
    <url><loc>{}/page-d</loc></url>
    <url><loc>{}/page-e</loc></url>
</urlset>"#,
        base, base, base, base, base, base
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;

    let output = cmd()
        .arg("--url")
        .arg(base)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--max-pages")
        .arg("2")
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert!(
        output.status.success(),
        "expected success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let md_files = t.find_files("md");
    assert!(
        md_files.len() <= 2,
        "expected at most 2 .md files with --max-pages 2, got {}",
        md_files.len()
    );
}

/// --exclude-pattern /page-b skips URLs matching that path pattern.
#[tokio::test]
async fn exclude_pattern_skips_matching_urls() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><a href=\"/page-a\">A</a><a href=\"/page-b\">B</a></body></html>",
        ))
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page A</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page B</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let sitemap_url = format!("{}/sitemap.xml", base);
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{}/</loc></url>
    <url><loc>{}/page-a</loc></url>
    <url><loc>{}/page-b</loc></url>
</urlset>"#,
        base, base, base
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;

    let output = cmd()
        .arg("--url")
        .arg(base)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--exclude-pattern")
        .arg("/page-b")
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert!(
        output.status.success(),
        "expected success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = t.server.received_requests().await.unwrap();
    let page_a_requests = requests
        .iter()
        .filter(|r| r.url.path() == "/page-a")
        .count();
    let page_b_requests = requests
        .iter()
        .filter(|r| r.url.path() == "/page-b")
        .count();

    assert_eq!(
        page_a_requests, 1,
        "expected 1 request to /page-a, got {}",
        page_a_requests
    );
    assert_eq!(
        page_b_requests, 0,
        "expected 0 requests to /page-b (excluded), got {}",
        page_b_requests
    );

    // Seed `/` and `/page-a` both produce a .md file; `/page-b` is excluded.
    let md_files = t.find_files("md");
    assert_eq!(
        md_files.len(),
        2,
        "expected 2 .md files (seed + /page-a), got {}",
        md_files.len()
    );
}

/// --include-pattern /page-a only scrapes URLs matching that path pattern.
#[tokio::test]
async fn include_pattern_only_scrapes_matching_urls() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><a href=\"/page-a\">A</a><a href=\"/page-b\">B</a></body></html>",
        ))
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page A</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page B</h1></article></body></html>"),
        )
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let sitemap_url = format!("{}/sitemap.xml", base);
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{}/</loc></url>
    <url><loc>{}/page-a</loc></url>
    <url><loc>{}/page-b</loc></url>
</urlset>"#,
        base, base, base
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;

    let output = cmd()
        .arg("--url")
        .arg(base)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--include-pattern")
        .arg("/page-a")
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert!(
        output.status.success(),
        "expected success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = t.server.received_requests().await.unwrap();
    let page_a_requests = requests
        .iter()
        .filter(|r| r.url.path() == "/page-a")
        .count();
    let page_b_requests = requests
        .iter()
        .filter(|r| r.url.path() == "/page-b")
        .count();

    assert_eq!(
        page_a_requests, 1,
        "expected 1 request to /page-a, got {}",
        page_a_requests
    );
    assert_eq!(
        page_b_requests, 0,
        "expected 0 requests to /page-b (not included), got {}",
        page_b_requests
    );

    // In sitemap mode the include pattern filters discovery results:
    // `/` does not match `/page-a`, so only `/page-a` is scraped → 1 .md file.
    let md_files = t.find_files("md");
    assert_eq!(
        md_files.len(),
        1,
        "expected 1 .md file (/page-a only, seed excluded by include pattern), got {}",
        md_files.len()
    );
}

/// Crawl mode with `--js-strategy static` must respect `--timeout-secs`.
///
/// End-to-end guard for the user-visible contract behind issue #280: a crawl
/// against a slow endpoint with `--timeout-secs 2` must fail fast (~2s), not
/// hang on a hardcoded timeout. Uses `--use-sitemap` so URL discovery fetches
/// the fast sitemap instead of DOM-scraping the slow seed (DOM discovery uses
/// a legacy client with its own 30s timeout, unrelated to this contract).
/// The engine-level fix (`Engine::with_js_strategy`) is covered directly by
/// `tests/engine_js_strategy_timeout_test.rs`.
#[tokio::test]
async fn crawl_js_strategy_respects_timeout_secs() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let slow_url = format!("{base}/slow");
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/slow</loc></url>
</urlset>"#
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;

    let out_path = t.out.path().to_path_buf();

    let start = Instant::now();
    let output = timeout(
        Duration::from_secs(15),
        tokio::task::spawn_blocking(move || {
            cmd()
                .arg("--url")
                .arg(&slow_url)
                .arg("--sitemap-url")
                .arg(&sitemap_url)
                .arg("--use-sitemap")
                .arg("--js-strategy")
                .arg("static")
                .arg("--timeout-secs")
                .arg("2")
                .arg("--max-depth")
                .arg("0")
                .arg("--output")
                .arg(out_path)
                .arg("--quiet")
                .output()
        }),
    )
    .await
    .expect("test must not hang")
    .expect("task must not panic")
    .expect("command must execute");

    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(10),
        "JS strategy timeout test should complete in under 10s, took {:?}",
        elapsed
    );
    assert!(
        !output.status.success(),
        "request should have timed out, but command succeeded"
    );
}
