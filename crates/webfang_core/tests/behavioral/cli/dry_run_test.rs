//! Dry-run mode: no files produced, no network requests.

use crate::BehavioralTest;
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn dry_run_produces_zero_files() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(0) // must not be called
        .named("dry-run must not fetch")
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
async fn dry_run_makes_zero_requests() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(0)
        .named("dry-run must not make any requests")
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
        0,
        "dry-run should make zero HTTP requests, got {}",
        requests.len()
    );
}

#[tokio::test]
async fn dry_run_with_single_page_still_produces_nothing() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(0)
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
