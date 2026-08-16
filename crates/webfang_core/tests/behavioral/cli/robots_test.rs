//! Robots.txt enforcement from CLI args.

use crate::assert_snapshot_redacted;
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
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/</loc></url>
    <url><loc>{base}/private/page</loc></url>
</urlset>"#
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

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><a href=\"/private/page\">Private</a></body></html>"),
        )
        .mount(&t.server)
        .await;

    Mock::given(method("GET"))
        .and(path("/private/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><h1>Secret</h1><p>hidden content only reachable when robots are ignored.</p></body></html>",
        ))
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
 <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
     <url><loc>{base}/</loc></url>
     <url><loc>{base}/private/page</loc></url>
 </urlset>"#
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;

    let output = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert!(
        output.status.success(),
        "expected success with --ignore-robots, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let requests = t.server.received_requests().await.unwrap();
    let private_requests = requests
        .iter()
        .filter(|r| r.url.path().starts_with("/private"))
        .count();
    let seed_requests = requests.iter().filter(|r| r.url.path() == "/").count();

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
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><h1>Seed</h1><p>blocked content</p></body></html>"),
        )
        .mount(&t.server)
        .await;

    // Sitemap lists the seed so discovery succeeds (default discovery is
    // sitemap-based); robots.txt enforcement then blocks it at scrape time.
    let base = t.server.uri();
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/</loc></url>
</urlset>"#
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
