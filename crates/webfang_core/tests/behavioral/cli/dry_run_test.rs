//! Dry-run mode: discovers URLs but produces no files and no scrape requests.

use crate::BehavioralTest;
use wiremock::matchers::method;
use wiremock::{Mock, ResponseTemplate};

/// Sets up the standard discovery mock and returns the BehavioralTest.
async fn setup_dry_run_test() -> BehavioralTest {
    let t = BehavioralTest::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(1) // one discovery request
        .named("dry-run discovery request")
        .mount(&t.server)
        .await;
    t
}

#[tokio::test]
async fn dry_run_produces_zero_files() {
    let t = setup_dry_run_test().await;

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
    let t = setup_dry_run_test().await;

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
    let t = setup_dry_run_test().await;

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

/// #1381: a `--dry-run` whose seed the SSRF guard cuts pre-socket must report the
/// refusal instead of a clean zero.
///
/// The guard opens no socket, so discovery completes `Ok` with zero URLs — the
/// engine's own `crawl completed … errors: 1` says so, but `discover_urls_unified`
/// returns only the URL list, so the preview used to print
/// `Dry-run: 0 URL(s) would be scraped:` and exit 0. Automation could not tell a
/// policy refusal from a site with no links. The sitemap null-result arms already
/// answer with exit 2; the refusal now does the same, naming its cause and the
/// documented hatch pair.
///
/// The harness arms the entry-guard disarmer for every spawned binary (wiremock
/// binds `127.0.0.1`), so this test opts back OUT of it — the production posture.
/// The mounted mock is a tripwire: a correct refusal never reaches it.
#[tokio::test]
async fn issue_1381_dry_run_refused_loopback_seed_exits_2_and_names_the_hatches() {
    let t = BehavioralTest::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .env_remove(webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV)
        .arg("--dry-run")
        .arg("--quiet")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("SSRF detectado"))
        .stderr(predicates::str::contains("WEBFANG_DISABLE_SSRF_RESOLVER"));

    let requests = t.server.received_requests().await.unwrap_or_default();
    assert!(
        requests.is_empty(),
        "the refusal is pre-socket, so the mock must receive nothing, got {requests:?}"
    );
}

/// Control for the same command: with the entry-guard hatch armed (the harness
/// default) the seed is fetched for real and a zero-URL preview keeps exiting 0.
/// Without this, "make the refusal visible" could silently become "make dry-run
/// fail on every empty site".
#[tokio::test]
async fn issue_1381_dry_run_with_the_hatch_armed_keeps_reporting_a_clean_zero() {
    let t = BehavioralTest::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--dry-run")
        .arg("--quiet")
        .assert()
        .success();
}
