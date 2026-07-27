//! Robots.txt enforcement from CLI args.

use crate::cmd;
use crate::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// A Disallow: /private/ in robots.txt prevents the crawler from
/// fetching /private/page; the seed page is still fetched.
#[tokio::test]
async fn robots_txt_disallow_prevents_fetching() {
    let t = BehavioralTest::new().await;

    // robots.txt disallows /private/
    crate::common::mock_robots(&t.server, "User-agent: *\nDisallow: /private/\n").await;

    // Seed page links to /private/page.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><a href=\"/private/page\">Private</a></body></html>"),
        )
        .mount(&t.server)
        .await;

    // The /private/page endpoint is not mocked; if the crawler respects
    // robots.txt it will never reach it and wiremock returns 404 by default.

    // Sitemap lists the seed and the disallowed URL so discovery succeeds
    // (default discovery is sitemap-based); robots.txt enforcement must still
    // block /private/page at scrape time.
    let base = t.server.uri();
    let sitemap_url = format!("{}/sitemap.xml", base);
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{}/</loc></url>
    <url><loc>{}/private/page</loc></url>
</urlset>"#,
        base, base
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;

    let output = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
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
    let private_requests = requests
        .iter()
        .filter(|r| r.url.path().starts_with("/private"))
        .count();
    let seed_requests = requests.iter().filter(|r| r.url.path() == "/").count();

    assert_eq!(
        private_requests, 0,
        "expected 0 requests to /private (disallowed by robots.txt), got {}",
        private_requests
    );
    assert_eq!(
        seed_requests, 1,
        "expected 1 request to seed /, got {}",
        seed_requests
    );
}
