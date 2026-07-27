//! Crawl behavior: depth limits, page caps, include/exclude patterns.

use crate::cmd;
use crate::BehavioralTest;
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
