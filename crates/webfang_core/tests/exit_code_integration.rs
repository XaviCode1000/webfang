//! Exit code integration tests
//!
//! Verifies that the CLI returns correct exit codes for:
//! - Empty sitemap discovery → exit 2 (EXIT_EMPTY_DISCOVERY)
//! - Network timeout → exit 69 (EXIT_UNAVAILABLE)
//! - Successful crawl → exit 0 (EXIT_SUCCESS)
//!
//! Run with: cargo nextest run --test-threads 2 exit_code_integration

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Resolve the path to the `webfang` binary.
///
/// `webfang` is built by `webfang_cli` (a workspace sibling), so
/// `assert_cmd::cargo_bin` cannot resolve it — `CARGO_BIN_EXE_webfang`
/// is only set for the owning crate.  This fallback searches
/// `target/{debug,release}` and builds the binary on demand.
fn webfang_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return PathBuf::from(p);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve workspace root");
    let target_root = std::env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("target"));
    for profile in ["debug", "release"] {
        let mut candidate = target_root.join(profile).join("webfang");
        if cfg!(windows) {
            candidate.set_extension("exe");
        }
        if candidate.exists() {
            return candidate;
        }
    }
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"])
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    let mut built = target_root.join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
}

fn cmd() -> Command {
    let mut c = Command::new(webfang_path());
    // Hermeticity: remove all WEBFANG_* / WEBFANG_AI_MODEL_ID / AI_MODEL_ID
    // env vars so poisoned CI environments (bug-discovery workflow) don't
    // affect arg parsing. Legacy `AI_MODEL_ID` is still honored by
    // `webfang_ai::infrastructure_ai::compat::read_ai_model_id_with` / `read_ai_model_id` (#980).
    let poisoned: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
        .map(|(k, _)| k)
        .collect();
    for key in poisoned {
        c.env_remove(&key);
    }
    c
}

// ============================================================================
// Tests: Empty sitemap → exit 2
// ============================================================================

/// Empty sitemap (no <loc> entries) returns exit code 2.
#[tokio::test]
async fn test_empty_sitemap_returns_exit_2() {
    let mock_server = MockServer::start().await;

    // Serve an empty sitemap
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#,
        ))
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let sitemap_url = format!("{}/sitemap.xml", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("No URLs discovered"));
}

/// Site without any sitemap (auto-discovery finds nothing) returns exit 2,
/// not exit 69: "no sitemap" is a terminal discovery state, not an
/// infrastructure failure (#695, OBS-SITEMAP-001).
#[tokio::test]
async fn test_missing_sitemap_returns_exit_2() {
    let mock_server = MockServer::start().await;

    // No mocks mounted: robots.txt and every sitemap candidate 404.
    let base_url = format!("{}/", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(2)
        .stderr(predicate::str::contains("No URLs discovered"));
}

// ============================================================================
// Tests: DOM discovery with only external links → exit 0 (seed injected)
// ============================================================================

/// DOM discovery that finds no internal URLs must still scrape the seed URL:
/// empty discovery is not fatal outside sitemap mode, so the run exits 0
/// instead of EmptyDiscovery (exit 2) (#488).
#[tokio::test]
async fn test_dom_external_only_links_scrapes_seed() {
    let mock_server = MockServer::start().await;

    // The seed page links only to an external domain, so DOM discovery yields
    // zero internal URLs.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><h1>Seed content</h1>\
             <p>The seed page carries plenty of substantive server-rendered text so it \
             comfortably clears the fifty character minimum content guard.</p>\
             <a href=\"https://iana.org\">external link</a></body></html>",
        ))
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let out_dir = tempfile::TempDir::new().expect("create temp output dir");

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--output")
        .arg(out_dir.path())
        .timeout(Duration::from_secs(30))
        .assert()
        .code(0);

    // The seed page itself must have been fetched: DOM discovery reads it once
    // for link extraction and the scraper reads it again for content.
    let requests = mock_server.received_requests().await.unwrap();
    let seed_requests = requests.iter().filter(|r| r.url.path() == "/").count();
    assert!(
        seed_requests >= 1,
        "expected the seed page / to be fetched, got {seed_requests} requests"
    );

    // The seed page was saved as markdown in the output dir and contains the
    // identifiable seed content.
    let md_files: Vec<PathBuf> = walkdir::WalkDir::new(out_dir.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| e.path().to_path_buf())
        .collect();
    assert_eq!(
        md_files.len(),
        1,
        "expected 1 .md file in output, got {}",
        md_files.len()
    );
    let content = std::fs::read_to_string(&md_files[0]).expect("read .md file");
    assert!(
        content.contains("Seed content"),
        "expected seed content in .md file"
    );
}

// ============================================================================
// Tests: Network timeout → exit 69
// ============================================================================

/// Timeout during sitemap fetch returns exit code 69.
#[tokio::test]
async fn test_timeout_returns_exit_69() {
    let mock_server = MockServer::start().await;

    // Serve a response with a very long delay to trigger timeout
    Mock::given(method("GET"))
        .and(path("/slow-sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>http://PLACEHOLDER/page1</loc></url>
</urlset>"#,
                )
                .set_delay(Duration::from_secs(120)),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());
    let sitemap_url = format!("{}/slow-sitemap.xml", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .arg("--timeout-secs")
        .arg("1")
        .timeout(Duration::from_secs(60))
        .assert()
        .code(69)
        .stderr(predicate::str::contains("URL discovery failed"));
}

// ============================================================================
// Tests: Successful discovery → exit 0
// ============================================================================

/// Valid sitemap with URLs returns exit code 0 (no regression).
#[tokio::test]
async fn test_valid_sitemap_returns_exit_0() {
    let mock_server = MockServer::start().await;
    let server_uri = mock_server.uri();

    // Serve a valid sitemap with one URL
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>{server_uri}/page1</loc></url>
</urlset>"#
        )))
        .mount(&mock_server)
        .await;

    // Serve the page content
    Mock::given(method("GET"))
        .and(path("/page1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><h1>Hello World</h1><p>Test content long enough to clear the \
                 fifty character minimum content guard comfortably.</p></body></html>",
        ))
        .mount(&mock_server)
        .await;

    let base_url = format!("{server_uri}/");
    let sitemap_url = format!("{server_uri}/sitemap.xml");

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--sitemap-url")
        .arg(&sitemap_url)
        .arg("--use-sitemap")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(0);
}

// ============================================================================
// Tests: --clean-ai preflight on non-AI builds (#761)
// ============================================================================

/// `--clean-ai` on a binary built without the `ai` feature must fail in
/// preflight with exit 78 BEFORE any network request — previously the CLI
/// scraped the whole page and only failed at export time (#761).
#[cfg(not(feature = "ai"))]
#[tokio::test]
async fn test_clean_ai_without_feature_fails_before_fetch() {
    let mock_server = MockServer::start().await;

    // If the CLI fetches anything, this mock would record it — the assertion
    // below proves the preflight fired before any request left the process.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                "<html><body><p>content that must never be fetched</p></body></html>",
            ),
        )
        .mount(&mock_server)
        .await;

    let base_url = format!("{}/", mock_server.uri());

    cmd()
        .arg("--url")
        .arg(&base_url)
        .arg("--clean-ai")
        .timeout(Duration::from_secs(30))
        .assert()
        .code(78)
        .stderr(predicate::str::contains("--clean-ai"));

    let requests = mock_server.received_requests().await.unwrap();
    assert!(
        requests.is_empty(),
        "preflight must fail before any network request, got {} requests",
        requests.len()
    );
}
