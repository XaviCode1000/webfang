//! Resume behavior: `--resume` skips already-processed URLs and persists
//! state; `--state-dir` redirects the state file away from the shared cache.
//!
//! Every test passes a fresh temp `--state-dir`. The state file is keyed by
//! host WITHOUT the port (`domain_from_url` → `127.0.0.1`), so all wiremock
//! runs collide on `127.0.0.1.json`; a temp dir per test prevents cross-run
//! contamination and never touches `~/.cache/webfang/state`.

use crate::BehavioralTest;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Host (no port) that `domain_from_url` derives from the wiremock origin —
/// the state file is named `<domain>.json`.
const DOMAIN: &str = "127.0.0.1";

/// Mount `/page1` and `/page2` plus a sitemap listing exactly those two URLs,
/// and return the sitemap URL. Sitemap mode keeps the discovered URL set
/// deterministic (no DOM-crawl seed injection).
async fn mount_resume_fixture(t: &BehavioralTest) -> String {
    Mock::given(method("GET"))
        .and(path("/page1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Page One</h1><p>Body one.</p></article></body></html>",
        ))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Page Two</h1><p>Body two.</p></article></body></html>",
        ))
        .mount(&t.server)
        .await;

    let base = t.server.uri();
    let sitemap_url = format!("{base}/sitemap.xml");
    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/page1</loc></url>
    <url><loc>{base}/page2</loc></url>
</urlset>"#
    );
    crate::common::mock_sitemap(&t.server, &sitemap_url, &sitemap_xml).await;
    sitemap_url
}

/// Count requests the mock server has received for a given URL path.
async fn count_path(server: &wiremock::MockServer, path: &str) -> usize {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == path)
        .count()
}

/// A second `--resume` run with the same state dir skips already-processed
/// URLs: the mock server receives no new page requests and the `.md` count is
/// unchanged.
#[tokio::test]
async fn resume_skips_already_processed_urls() {
    let t = BehavioralTest::new().await;
    let sitemap_url = mount_resume_fixture(&t).await;
    let state_dir = TempDir::new().unwrap();

    let run1 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("run binary");
    assert!(
        run1.status.success(),
        "first run should succeed: {}",
        String::from_utf8_lossy(&run1.stderr)
    );

    assert_eq!(
        count_path(&t.server, "/page1").await,
        1,
        "run1 fetches /page1"
    );
    assert_eq!(
        count_path(&t.server, "/page2").await,
        1,
        "run1 fetches /page2"
    );
    let md_after_run1 = t.find_files("md").len();
    assert!(
        md_after_run1 >= 2,
        "first run should produce at least 2 .md files, got {md_after_run1}"
    );

    // State persisted to the temp state dir.
    assert!(
        state_dir.path().join(format!("{DOMAIN}.json")).exists(),
        "state file must persist after the first run"
    );

    // Second run, same state dir: every URL is already processed, so nothing
    // is re-fetched and the output is untouched. (The run exits non-zero
    // because an empty scrape list is a hard error — assert on counts, not
    // exit success.)
    let _run2 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert_eq!(
        count_path(&t.server, "/page1").await,
        1,
        "resume must not re-fetch /page1"
    );
    assert_eq!(
        count_path(&t.server, "/page2").await,
        1,
        "resume must not re-fetch /page2"
    );
    assert_eq!(
        t.find_files("md").len(),
        md_after_run1,
        "resume must leave the .md count unchanged"
    );
}

/// `--state-dir` writes the state JSON into the supplied directory and never
/// into the default cache location.
#[tokio::test]
async fn state_dir_uses_custom_directory() {
    let t = BehavioralTest::new().await;
    let sitemap_url = mount_resume_fixture(&t).await;
    let state_dir = TempDir::new().unwrap();
    // Redirect the default-cache base so we can prove `--state-dir` wins: if
    // the flag were ignored, the state would land under this fake XDG cache.
    let fake_cache = TempDir::new().unwrap();

    let run1 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .env("XDG_CACHE_HOME", fake_cache.path())
        .output()
        .expect("run binary");
    assert!(
        run1.status.success(),
        "run should succeed: {}",
        String::from_utf8_lossy(&run1.stderr)
    );

    assert!(
        state_dir.path().join(format!("{DOMAIN}.json")).exists(),
        "state file must live in the custom --state-dir"
    );
    assert!(
        !fake_cache
            .path()
            .join("webfang")
            .join("state")
            .join(format!("{DOMAIN}.json"))
            .exists(),
        "state file must NOT be written to the default cache when --state-dir is set"
    );

    // Second run with the same custom dir skips the processed URLs.
    let _run2 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .env("XDG_CACHE_HOME", fake_cache.path())
        .output()
        .expect("run binary");

    assert_eq!(
        count_path(&t.server, "/page1").await,
        1,
        "resume must not re-fetch /page1"
    );
    assert_eq!(
        count_path(&t.server, "/page2").await,
        1,
        "resume must not re-fetch /page2"
    );
}

/// State persists in `--state-dir` while results go to `--output`; a second
/// run with the same state dir but a cleaned output dir still skips processed
/// URLs (state, not output, drives resume).
#[tokio::test]
async fn resume_state_and_output_in_different_directories() {
    let t = BehavioralTest::new().await;
    let sitemap_url = mount_resume_fixture(&t).await;
    let state_dir = TempDir::new().unwrap();

    let run1 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("run binary");
    assert!(
        run1.status.success(),
        "first run should succeed: {}",
        String::from_utf8_lossy(&run1.stderr)
    );

    assert!(
        state_dir.path().join(format!("{DOMAIN}.json")).exists(),
        "state must persist in the state dir"
    );
    assert!(
        t.find_files("md").len() >= 2,
        "results must land in the output dir"
    );

    // Clean the output dir; keep the state dir.
    for file in t.find_files("md") {
        std::fs::remove_file(file).expect("remove .md file");
    }
    assert!(t.find_files("md").is_empty(), "output dir cleaned");

    // Second run: same state dir, empty output. Resume skips the processed
    // URLs, so nothing is re-scraped and the output stays empty.
    let _run2 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert_eq!(
        count_path(&t.server, "/page1").await,
        1,
        "resume must not re-fetch /page1 with a clean output dir"
    );
    assert_eq!(
        count_path(&t.server, "/page2").await,
        1,
        "resume must not re-fetch /page2 with a clean output dir"
    );
    assert!(
        t.find_files("md").is_empty(),
        "no re-scrape means no new .md files in the cleaned output dir"
    );
}

/// A corrupt state JSON does not crash the run: `apply_resume_mode` falls back
/// to the full URL list, so every URL is still scraped and saved.
#[tokio::test]
async fn corrupt_state_falls_back_to_full_scrape() {
    let t = BehavioralTest::new().await;
    let sitemap_url = mount_resume_fixture(&t).await;
    let state_dir = TempDir::new().unwrap();

    std::fs::write(
        state_dir.path().join(format!("{DOMAIN}.json")),
        "not valid json!!!",
    )
    .expect("write corrupt state");

    let output = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("run binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked"),
        "corrupt state must not panic: {stderr}"
    );

    assert_eq!(
        count_path(&t.server, "/page1").await,
        1,
        "corrupt state must still scrape /page1"
    );
    assert_eq!(
        count_path(&t.server, "/page2").await,
        1,
        "corrupt state must still scrape /page2"
    );
    assert!(
        t.find_files("md").len() >= 2,
        "corrupt state must still produce .md files"
    );
}

/// A `--resume` run where every discovered URL was already processed by a prior
/// run is a technical success, not a failure: it must exit 0 with a "nothing
/// pending" message instead of the false network-error exit 69 (#705 Paso 2).
#[tokio::test]
async fn resume_all_processed_exits_zero() {
    let t = BehavioralTest::new().await;
    let sitemap_url = mount_resume_fixture(&t).await;
    let state_dir = TempDir::new().unwrap();

    // First run processes every URL and persists state.
    let run1 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("run binary");
    assert!(
        run1.status.success(),
        "first run should succeed: {}",
        String::from_utf8_lossy(&run1.stderr)
    );

    // Second run with the same state dir: every URL is already processed, so
    // there is nothing pending. This is a success, not a network error.
    let run2 = t
        .state_dir_cmd(state_dir.path())
        .arg("--use-sitemap")
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert!(
        run2.status.success(),
        "resume with nothing pending must exit 0, got {:?}: {}",
        run2.status.code(),
        String::from_utf8_lossy(&run2.stderr)
    );
}
