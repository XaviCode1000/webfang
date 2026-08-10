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
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/</loc></url>
    <url><loc>{base}/page-a</loc></url>
</urlset>"#
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
        "expected 1 request to seed /, got {seed_requests}"
    );
    assert_eq!(
        page_a_requests, 0,
        "expected 0 requests to /page-a with max-depth 0, got {page_a_requests}"
    );

    let md_files = t.find_files("md");
    assert_eq!(
        md_files.len(),
        1,
        "expected 1 .md file, got {}",
        md_files.len()
    );
}

/// Mount a sub-path sitemap fallback scenario and return the server base URL:
/// - main `/sitemap.xml` lists only a URL outside `/blog/`, so discovery finds
///   no relevant URLs and falls back to sub-path sitemaps;
/// - `/blog/sitemap.xml` (HEAD probe + GET parse) lists two `/blog/` content URLs;
/// - the two content pages return scrapeable HTML.
async fn mount_subpath_scenario(server: &wiremock::MockServer) -> String {
    let base = server.uri();

    crate::common::mock_robots(server, "User-agent: *\n").await;

    let main_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/about</loc></url>
</urlset>"#
    );
    crate::common::mock_sitemap(server, &format!("{base}/sitemap.xml"), &main_xml).await;

    // The fallback probes candidates with HEAD before parsing them with GET.
    Mock::given(method("HEAD"))
        .and(path("/blog/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;

    let subpath_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/blog/post-1</loc></url>
    <url><loc>{base}/blog/post-2</loc></url>
</urlset>"#
    );
    crate::common::mock_sitemap(server, &format!("{base}/blog/sitemap.xml"), &subpath_xml).await;

    Mock::given(method("GET"))
        .and(path("/blog/post-1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Post 1</h1></article></body></html>"),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/blog/post-2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Post 2</h1></article></body></html>"),
        )
        .mount(server)
        .await;

    base
}

/// --max-depth 0 on the sub-path sitemap fallback scrapes none of the
/// sub-sitemap URLs: they are depth 1, so the crawl's max_depth gate filters
/// them all out and discovery comes back empty (EmptyDiscovery exit).
#[tokio::test]
async fn subpath_sitemap_max_depth_zero_skips_content() {
    let t = BehavioralTest::new().await;
    let base = mount_subpath_scenario(&t.server).await;

    let output = cmd()
        .arg("--url")
        .arg(format!("{base}/blog/"))
        .arg("--sitemap-url")
        .arg(format!("{base}/sitemap.xml"))
        .arg("--use-sitemap")
        .arg("--max-depth")
        .arg("0")
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    // No scrapeable URL survives the depth gate -> EmptyDiscovery (non-success).
    assert!(
        !output.status.success(),
        "expected empty-discovery failure, got success; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = t.server.received_requests().await.unwrap();
    let post1 = requests
        .iter()
        .filter(|r| r.url.path() == "/blog/post-1")
        .count();
    let post2 = requests
        .iter()
        .filter(|r| r.url.path() == "/blog/post-2")
        .count();
    assert_eq!(
        post1, 0,
        "expected 0 requests to /blog/post-1 with max-depth 0, got {post1}"
    );
    assert_eq!(
        post2, 0,
        "expected 0 requests to /blog/post-2 with max-depth 0, got {post2}"
    );
}

/// Control for `subpath_sitemap_max_depth_zero_skips_content`: with --max-depth 1
/// the same sub-path sitemap URLs (depth 1) ARE discovered and scraped, proving
/// the zero above comes from the depth gate and not a broken fallback.
#[tokio::test]
async fn subpath_sitemap_max_depth_one_scrapes_content() {
    let t = BehavioralTest::new().await;
    let base = mount_subpath_scenario(&t.server).await;

    let output = cmd()
        .arg("--url")
        .arg(format!("{base}/blog/"))
        .arg("--sitemap-url")
        .arg(format!("{base}/sitemap.xml"))
        .arg("--use-sitemap")
        .arg("--max-depth")
        .arg("1")
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
    let post1 = requests
        .iter()
        .filter(|r| r.url.path() == "/blog/post-1")
        .count();
    let post2 = requests
        .iter()
        .filter(|r| r.url.path() == "/blog/post-2")
        .count();
    assert!(
        post1 >= 1,
        "expected >=1 request to /blog/post-1 with max-depth 1, got {post1}"
    );
    assert!(
        post2 >= 1,
        "expected >=1 request to /blog/post-2 with max-depth 1, got {post2}"
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
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/</loc></url>
    <url><loc>{base}/page-a</loc></url>
    <url><loc>{base}/page-b</loc></url>
    <url><loc>{base}/page-c</loc></url>
    <url><loc>{base}/page-d</loc></url>
    <url><loc>{base}/page-e</loc></url>
</urlset>"#
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
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/</loc></url>
    <url><loc>{base}/page-a</loc></url>
    <url><loc>{base}/page-b</loc></url>
</urlset>"#
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
        "expected 1 request to /page-a, got {page_a_requests}"
    );
    assert_eq!(
        page_b_requests, 0,
        "expected 0 requests to /page-b (excluded), got {page_b_requests}"
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
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/</loc></url>
    <url><loc>{base}/page-a</loc></url>
    <url><loc>{base}/page-b</loc></url>
</urlset>"#
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
        "expected 1 request to /page-a, got {page_a_requests}"
    );
    assert_eq!(
        page_b_requests, 0,
        "expected 0 requests to /page-b (not included), got {page_b_requests}"
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
        "JS strategy timeout test should complete in under 10s, took {elapsed:?}"
    );
    assert!(
        !output.status.success(),
        "request should have timed out, but command succeeded"
    );
}

/// Mount a two-level DOM scenario used by the `--max-depth` gating tests:
/// - `/` links to `/page1` and `/page2`;
/// - `/page1` links to `/deep`;
/// - `/page2` and `/deep` return minimal HTML.
async fn mount_dom_depth_scenario(server: &wiremock::MockServer) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><a href=\"/page1\">Page 1</a><a href=\"/page2\">Page 2</a></body></html>",
        ))
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "<html><body><a href=\"/deep\">Deep</a><h1>Page 1</h1></body></html>",
            ),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Page 2</h1></article></body></html>"),
        )
        .mount(server)
        .await;

    Mock::given(method("GET"))
        .and(path("/deep"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Deep</h1></article></body></html>"),
        )
        .mount(server)
        .await;
}

/// Regression for bug #651: with `--max-depth 1` the recursive crawl must NOT
/// fetch URLs beyond the first hop, so `/deep` (reachable only via `/page1`)
/// is never requested. This proves `--max-depth` is now honored in the default
/// (non-sitemap, non-interactive) DOM crawl.
#[tokio::test]
async fn max_depth_one_excludes_deeper_links() {
    let t = BehavioralTest::new().await;
    mount_dom_depth_scenario(&t.server).await;

    let output = cmd()
        .arg("--url")
        .arg(t.server.uri())
        .arg("--ignore-robots")
        .arg("--max-depth")
        .arg("1")
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
    let deep_requests = requests.iter().filter(|r| r.url.path() == "/deep").count();
    let page1_requests = requests.iter().filter(|r| r.url.path() == "/page1").count();
    let page2_requests = requests.iter().filter(|r| r.url.path() == "/page2").count();

    assert_eq!(
        deep_requests, 0,
        "expected 0 requests to /deep with max-depth 1, got {deep_requests}"
    );
    assert!(
        page1_requests >= 1,
        "expected /page1 to be crawled, got {page1_requests}"
    );
    assert!(
        page2_requests >= 1,
        "expected /page2 to be crawled, got {page2_requests}"
    );

    let md_files = t.find_files("md");
    // Seed `/`, `/page1`, `/page2` — but NOT `/deep`.
    assert_eq!(
        md_files.len(),
        3,
        "expected 3 .md files (seed + page1 + page2), got {}",
        md_files.len()
    );
}

/// Control for `max_depth_one_excludes_deeper_links`: with `--max-depth 2` the
/// same scenario must fetch `/deep`, proving the gate is depth-driven and not a
/// hard cap on the crawl.
#[tokio::test]
async fn max_depth_two_includes_deeper_links() {
    let t = BehavioralTest::new().await;
    mount_dom_depth_scenario(&t.server).await;

    let output = cmd()
        .arg("--url")
        .arg(t.server.uri())
        .arg("--ignore-robots")
        .arg("--max-depth")
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

    let requests = t.server.received_requests().await.unwrap();
    let deep_requests = requests.iter().filter(|r| r.url.path() == "/deep").count();

    assert!(
        deep_requests >= 1,
        "expected >=1 request to /deep with max-depth 2, got {deep_requests}"
    );

    let md_files = t.find_files("md");
    // Seed `/`, `/page1`, `/page2`, `/deep`.
    assert_eq!(
        md_files.len(),
        4,
        "expected 4 .md files (seed + page1 + page2 + deep), got {}",
        md_files.len()
    );
}
