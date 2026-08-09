//! Dry-run mode: discovers URLs but produces no files and no scrape requests.

use crate::BehavioralTest;
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn dry_run_produces_zero_files() {
    let t = BehavioralTest::new().await;

    // Mock the seed URL for discovery (returns minimal HTML)
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(1) // one discovery request
        .named("dry-run discovery request")
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--dry-run")
        .arg("--quiet")
        .assert()
        .success();

    let entries: Vec<_> = std::fs::read_dir(t.out.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "dry-run must not create output files, found {}",
        entries.len()
    );
}

#[tokio::test]
async fn dry_run_makes_discovery_request_only() {
    let t = BehavioralTest::new().await;

    // Mock the seed URL for discovery (returns minimal HTML)
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(1) // one discovery request, no scrape requests
        .named("dry-run discovery request only")
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--dry-run")
        .arg("--quiet")
        .assert()
        .success();

    let requests = t.server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "dry-run should make exactly one discovery request, got {}",
        requests.len()
    );
}

#[tokio::test]
async fn dry_run_with_single_page_still_produces_nothing() {
    let t = BehavioralTest::new().await;

    // Mock the seed URL for discovery (returns minimal HTML)
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(1) // one discovery request
        .named("dry-run discovery request")
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--dry-run")
        .arg("--quiet")
        .assert()
        .success();

    let entries: Vec<_> = std::fs::read_dir(t.out.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "dry-run + single-page should produce no files"
    );
}
