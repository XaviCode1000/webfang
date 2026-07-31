//! Sitemap-based discovery: --use-sitemap with explicit --sitemap-url.

use crate::cmd;
use tempfile::TempDir;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// --sitemap-url with --use-sitemap fetches the explicit sitemap URL and
/// scrapes the URLs listed in it.
#[tokio::test]
async fn sitemap_url_scrapes_listed_urls() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Seed page (may or may not be fetched — sitemap discovery takes precedence)
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Seed Page</h1>\
                 <p>Seed content.</p>\
                 </article></body></html>",
        ))
        .mount(&server)
        .await;

    // robots.txt (empty — allow all)
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
        .mount(&server)
        .await;

    // Explicit sitemap listing two pages
    let base = server.uri();
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/page-a</loc></url>
    <url><loc>{base}/page-b</loc></url>
</urlset>"#,
        )))
        .mount(&server)
        .await;

    // Pages listed in the sitemap
    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Page A</h1>\
                 <p>Content from sitemap page A.</p>\
                 </article></body></html>",
        ))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Page B</h1>\
                 <p>Content from sitemap page B.</p>\
                 </article></body></html>",
        ))
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(format!("{}/sitemap.xml", server.uri()))
        .arg("--output")
        .arg(output.path())
        .arg("--max-pages")
        .arg("5")
        .arg("--quiet")
        .assert()
        .success();

    // Verify both pages from the sitemap were scraped
    let all_content: String = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect();

    assert!(
        all_content.contains("Page A"),
        "output should contain content from sitemap page A"
    );
    assert!(
        all_content.contains("Page B"),
        "output should contain content from sitemap page B"
    );
}
