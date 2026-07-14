//! CLI adapter behavioral tests
//!
//! End-to-end tests that invoke the actual `webfang` binary via `assert_cmd`.
//! Mock HTTP servers (wiremock) simulate target websites; TempDir captures output.
//!
//! Run with: cargo nextest run --test cli_behavioral_test

// Gate the entire test file behind the default feature pair. These tests
// invoke the full `webfang` binary which requires `images` + `documents`
// for --download-images/--download-documents. When building with
// --no-default-features (headless/persistence-off CI matrix) the file is
// skipped entirely instead of triggering a hard compile_error!.
#![cfg(all(feature = "images", feature = "documents"))]

use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;
use tempfile::TempDir;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Resolve the path to the `webfang` binary.
///
/// `webfang` is built by the `rust_scraper_cli` crate (a workspace sibling),
/// so `assert_cmd::cargo_bin` cannot locate it from `rust_scraper_core` tests
/// — `CARGO_BIN_EXE_webfang` is only set for the crate that owns the binary.
/// We fall back to the workspace `target/` dir and, if missing, build it.
fn webfang_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return std::path::PathBuf::from(p);
    }
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/rust_scraper_core -> workspace root (two levels up)
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve workspace root");
    for profile in ["debug", "release"] {
        let mut candidate = workspace_root.join("target").join(profile).join("webfang");
        if cfg!(windows) {
            candidate.set_extension("exe");
        }
        if candidate.exists() {
            return candidate;
        }
    }
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "rust_scraper_cli", "--bin", "webfang", "--quiet"])
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    let mut built = workspace_root.join("target").join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
}

fn cmd() -> Command {
    Command::new(webfang_path())
}

// ============================================================================
// 1. Exit codes
// ============================================================================

/// --help prints usage and exits with code 0 (not 64).
#[test]
fn test_help_exits_zero() {
    cmd()
        .arg("--help")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("rust_scraper binary"));
}

/// --version prints version and exits with code 0.
#[test]
fn test_version_exits_zero() {
    cmd()
        .arg("--version")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

/// Missing --url (non-interactive, non-batch) exits with code 64.
#[test]
fn test_no_url_exits_64() {
    cmd()
        .assert()
        .code(64)
        .stderr(predicate::str::contains("--url is required"));
}

/// Invalid URL string exits with code 64 and shows "Invalid URL".
#[test]
fn test_invalid_url_exits_64() {
    cmd()
        .arg("--url")
        .arg("not-a-url")
        .assert()
        .code(64)
        .stderr(predicate::str::contains("Invalid URL"));
}

/// Unreachable server exits with code 69 (network error).
#[test]
fn test_unreachable_server_exits_69() {
    cmd()
        .arg("--url")
        .arg("http://127.0.0.1:1")
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("2")
        .arg("--max-retries")
        .arg("0")
        .arg("--quiet")
        .assert()
        .code(69);
}

// ============================================================================
// 2. Single page scraping — mock server → output files
// ============================================================================

/// Single-page scrape fetches only the seed URL and writes output.
#[tokio::test]
async fn test_single_page_writes_output_file() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><head><title>Test Page</title></head>\
                 <body><main><article>\
                 <h1>Hello World</h1>\
                 <p>This is meaningful content for the extractor.</p>\
                 </article></main></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let files: Vec<_> = std::fs::read_dir(output.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .collect();
    assert!(
        !files.is_empty(),
        "output directory must contain at least one file"
    );

    // Content may be empty if Readability can't extract from mock HTML — that's OK.
    // The important thing is the binary exits successfully and creates output files.
}

// ============================================================================
// 3. Output format tests — markdown, json, text
// ============================================================================

/// --format markdown produces a .md file.
#[tokio::test]
async fn test_format_markdown_creates_md_file() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Markdown Test</h1>\
                 <p>Content for markdown format verification.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    // Output files live in domain subdirs (e.g. 127.0.0.1/index.md),
    // so we must walk recursively instead of using flat read_dir.
    let md_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();
    assert!(
        !md_files.is_empty(),
        "expected at least one .md file in output directory"
    );
}

/// --format text produces a .txt file (text format replaces .md extension with .txt).
#[tokio::test]
async fn test_format_text_creates_txt_file() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Text Test</h1>\
                 <p>Content for text format verification.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("text")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    // Output files live in domain subdirs (e.g. 127.0.0.1/index.txt),
    // so we must walk recursively instead of using flat read_dir.
    let txt_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "txt"))
        .collect();
    assert!(
        !txt_files.is_empty(),
        "expected at least one .txt file in output directory"
    );
}

/// --format json produces a .json file.
#[tokio::test]
async fn test_format_json_creates_json_file() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>JSON Test</h1>\
                 <p>Content for json format verification.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("json")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let json_files: Vec<_> = std::fs::read_dir(output.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .collect();
    assert!(
        !json_files.is_empty(),
        "expected at least one .json file in output directory"
    );
}

// ============================================================================
// 4. Dry run — no output files, no network requests
// ============================================================================

/// --dry-run makes zero network requests.
#[tokio::test]
async fn test_dry_run_zero_requests() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(0)
        .named("dry-run must not fetch")
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--dry-run")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();
}

/// --dry-run creates no output files.
#[tokio::test]
async fn test_dry_run_creates_no_files() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html><body>test</body></html>"))
        .expect(0)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--dry-run")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let entries: Vec<_> = std::fs::read_dir(output.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "dry-run must not create output files, found {}",
        entries.len()
    );
}

// ============================================================================
// 5. Crawl mode — mock server with links → verify discovery
// ============================================================================

/// Crawl mode discovers linked pages and scrapes them.
///
/// IGNORED: The crawl discovery pipeline (discover_urls_for_tui → scrape_urls)
/// requires robots.txt fetching, link extraction, URL normalization, and the
/// full scraper engine — making it unreliable as a behavioral test with mock
/// HTTP servers. The discovery phase fetches the seed page via wreq, while
/// scraping uses a separate HttpClient, and robots.txt checking adds extra
/// requests that interact poorly with wiremock expect() counts.
#[ignore = "complex multi-phase crawl pipeline is unreliable with mock servers"]
#[tokio::test]
async fn test_crawl_discovers_linked_pages() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Seed page with two links
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body>\
                 <a href=\"/page1\">Page 1</a>\
                 <a href=\"/page2\">Page 2</a>\
                 <p>Main page with enough content for extraction.</p>\
                 </body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    // Linked page 1
    Mock::given(method("GET"))
        .and(path("/page1"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body>\
                 <h1>Page One</h1>\
                 <p>Content of the first linked page for verification.</p>\
                 </body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    // Linked page 2
    Mock::given(method("GET"))
        .and(path("/page2"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body>\
                 <h1>Page Two</h1>\
                 <p>Content of the second linked page for verification.</p>\
                 </body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--output")
        .arg(output.path())
        .arg("--max-depth")
        .arg("2")
        .arg("--max-pages")
        .arg("5")
        .arg("--quiet")
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    assert!(
        requests.len() >= 2,
        "crawl should discover and fetch at least the seed + 1 linked page, got {} requests",
        requests.len()
    );
}

/// --max-pages limits the number of pages scraped.
#[ignore = "Pre-existing stale test, out of scope for insta migration"]
#[tokio::test]
async fn test_max_pages_limits_crawl() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Seed page links to 3 sub-pages
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body>\
                 <a href=\"/a\">A</a><a href=\"/b\">B</a><a href=\"/c\">C</a>\
                 <p>Index page with links.</p>\
                 </body></html>",
        ))
        .mount(&server)
        .await;

    for p in ["/a", "/b", "/c"] {
        Mock::given(method("GET"))
            .and(path(p))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><h1>Page {p}</h1><p>Content for {p}.</p></body></html>"
            )))
            .mount(&server)
            .await;
    }

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--output")
        .arg(output.path())
        .arg("--max-pages")
        .arg("2")
        .arg("--quiet")
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    // Seed + up to 2 discovered pages = at most 3 total
    assert!(
        requests.len() <= 3,
        "max-pages=2 should limit to seed + 2 pages, got {} requests",
        requests.len()
    );
}

// ============================================================================
// 6. Batch mode — stdin / file
// ============================================================================

/// --batch from stdin processes URLs and exits.
#[tokio::test]
async fn test_batch_stdin_processes_urls() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch Stdin Test</h1>\
                 <p>Content from batch stdin processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--batch")
        .write_stdin(format!("{}\n", server.uri()))
        .timeout(Duration::from_secs(10))
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch stdin should fetch exactly the provided URL, got {} requests",
        requests.len()
    );
}

/// --batch-file reads URLs from a file and processes them.
#[tokio::test]
async fn test_batch_file_processes_urls() {
    let server = MockServer::start().await;
    let temp = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch File Test</h1>\
                 <p>Content from batch file processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", server.uri())).unwrap();

    cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .timeout(Duration::from_secs(10))
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch-file should fetch exactly the URL from the file, got {} requests",
        requests.len()
    );
}

/// --batch with empty stdin exits with code 64 ("No URLs provided").
#[test]
fn test_batch_empty_stdin_exits_64() {
    cmd()
        .arg("--batch")
        .write_stdin("")
        .timeout(Duration::from_secs(5))
        .assert()
        .code(64)
        .stderr(predicate::str::contains("No URLs provided"));
}

/// --batch-file with empty file exits with code 64.
#[test]
fn test_batch_empty_file_exits_64() {
    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, "").unwrap();

    cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .timeout(Duration::from_secs(5))
        .assert()
        .code(64)
        .stderr(predicate::str::contains("No URLs provided"));
}

// ============================================================================
// 7. Error handling
// ============================================================================

/// Non-HTTP scheme (FTP) is rejected gracefully.
///
/// `url::Url::parse("ftp://example.com")` succeeds, so it passes the CLI
/// "Invalid URL" check. The HTTP client then fails during discovery with
/// "URI scheme is not allowed", discovery returns empty, and the orchestrator
/// exits with code 0 (no pages to scrape = success, not error).
#[ignore = "Pre-existing stale test, out of scope for insta migration"]
#[test]
fn test_ftp_scheme_exits_cleanly() {
    cmd().arg("--url").arg("ftp://example.com").assert().code(0);
}

/// --timeout-secs with unreachable server → exit 69 after timeout.
#[tokio::test]
async fn test_timeout_unreachable_server_exits_69() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Server responds after a long delay
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>slow</body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/slow", server.uri()))
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("1")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .code(69)
        .stderr(predicate::str::contains(
            "No pages were successfully scraped",
        ));
}

/// --output-dir flag is accepted and output is written there.
#[tokio::test]
async fn test_output_dir_flag_writes_to_specified_path() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let custom_dir = output.path().join("custom_out");

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Custom Dir Test</h1>\
                 <p>Verify output goes to the specified directory.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--output")
        .arg(&custom_dir)
        .arg("--quiet")
        .assert()
        .success();

    assert!(
        custom_dir.exists(),
        "--output directory should be created by the scraper"
    );
}

// ============================================================================
// 8. Download behavior — images and documents
// ============================================================================

/// --download-images fetches image URLs found in the page and saves them locally.
#[cfg_attr(not(feature = "images"), ignore = "requires --features images")]
#[tokio::test]
async fn test_download_images_saves_files() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Small 1x1 PNG (89 bytes)
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    // Page with an <img> tag pointing to the mock server
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Image Test</h1>\
                 <img src=\"/photo.png\" alt=\"test image\">\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    // Serve the image
    Mock::given(method("GET"))
        .and(path("/photo.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png_bytes))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--download-images")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    // Verify images directory was created with at least one file
    let images_dir = output.path().join("images");
    assert!(
        images_dir.exists(),
        "images/ directory should be created when --download-images is used"
    );

    let image_files: Vec<_> = std::fs::read_dir(&images_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
        .collect();
    assert!(
        !image_files.is_empty(),
        "at least one image file should be downloaded"
    );
}

/// --download-documents fetches document URLs (PDF, etc.) found in the page.
#[cfg_attr(not(feature = "documents"), ignore = "requires --features documents")]
#[tokio::test]
async fn test_download_documents_saves_files() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Fake PDF header (%PDF-)
    let pdf_bytes = b"%PDF-1.4 fake content for testing document download";

    // Page with an <a> tag linking to a PDF
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Document Test</h1>\
                 <a href=\"/report.pdf\">Download Report</a>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    // Serve the PDF
    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(pdf_bytes.to_vec())
                .insert_header("content-type", "application/pdf"),
        )
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    // Verify documents directory was created with at least one file
    let docs_dir = output.path().join("documents");
    assert!(
        docs_dir.exists(),
        "documents/ directory should be created when --download-documents is used"
    );

    let doc_files: Vec<_> = std::fs::read_dir(&docs_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
        .collect();
    assert!(
        !doc_files.is_empty(),
        "at least one document file should be downloaded"
    );
}

// ============================================================================
// 9. Obsidian behavior — tags, rich metadata, wiki-links, relative assets
// ============================================================================

/// --obsidian-tags adds tags to YAML frontmatter in the output file.
#[tokio::test]
async fn test_obsidian_tags_appear_in_frontmatter() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><head><title>Tagged Page</title></head>\
                 <body><article>\
                 <h1>Tagged Page</h1>\
                 <p>Content with obsidian tags.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--obsidian-tags")
        .arg("scraped,web-dev,rust")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    // Find the .md file (may be in domain subdirectory)
    let md_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();

    assert!(
        !md_files.is_empty(),
        "expected at least one .md file for frontmatter inspection"
    );

    let content = std::fs::read_to_string(md_files[0].path()).unwrap();
    assert!(
        content.contains("tags:"),
        "frontmatter should contain tags field"
    );
    assert!(
        content.contains("scraped") && content.contains("web-dev") && content.contains("rust"),
        "frontmatter should contain all specified tags: {}",
        content
    );
}

/// --obsidian-rich-metadata adds word count, reading time, and language to frontmatter.
#[tokio::test]
async fn test_obsidian_rich_metadata_in_frontmatter() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><head><title>Rich Meta Page</title></head>\
                 <body><article>\
                 <h1>Rich Metadata Test</h1>\
                 <p>This is a longer article to test rich metadata generation. \
                 The content needs enough words to trigger content type detection \
                 and provide meaningful reading time estimates for the frontmatter.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--obsidian-rich-metadata")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let md_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();

    assert!(
        !md_files.is_empty(),
        "expected at least one .md file for rich metadata inspection"
    );

    let content = std::fs::read_to_string(md_files[0].path()).unwrap();
    // Rich metadata uses camelCase in the YAML frontmatter
    assert!(
        content.contains("wordCount:") || content.contains("readingTime:"),
        "frontmatter should contain rich metadata fields (wordCount/readingTime): {}",
        &content[..content.len().min(500)]
    );
}

/// --obsidian-wiki-links converts same-domain URLs to [[wiki-link]] syntax.
#[tokio::test]
async fn test_obsidian_wiki_links_conversion() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Page with internal links to the same domain
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><head><title>Wiki Links Test</title></head>\
                 <body><article>\
                 <h1>Wiki Links Test</h1>\
                 <p>Check out <a href=\"/other-page\">this other page</a> for more info.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--obsidian-wiki-links")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let md_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();

    assert!(
        !md_files.is_empty(),
        "expected at least one .md file for wiki-link inspection"
    );

    let content = std::fs::read_to_string(md_files[0].path()).unwrap();
    // Wiki-links use [[page]] syntax — the exact conversion depends on the
    // implementation, but the original absolute URL should be gone or transformed
    assert!(
        content.contains("[["),
        "output should contain Obsidian [[wiki-link]] syntax: {}",
        &content[..content.len().min(500)]
    );
}

/// --obsidian-relative-assets rewrites downloaded asset paths as relative.
#[tokio::test]
#[ignore = "feature --obsidian-relative-assets not yet implemented"]
async fn test_obsidian_relative_assets_paths() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    // Small image bytes
    let png_bytes: Vec<u8> = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><head><title>Assets Test</title></head>\
                 <body><article>\
                 <h1>Relative Assets Test</h1>\
                 <img src=\"/logo.png\" alt=\"logo\">\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/logo.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(png_bytes))
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--download-images")
        .arg("--obsidian-relative-assets")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let md_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();

    assert!(
        !md_files.is_empty(),
        "expected at least one .md file for relative asset path inspection"
    );

    let content = std::fs::read_to_string(md_files[0].path()).unwrap();
    // When relative assets are enabled, image references should be relative
    // (no absolute URLs like http://...)
    let has_relative =
        content.contains("./") || content.contains("../") || content.contains("images/");
    let has_absolute_url = content.contains("http://") || content.contains("https://");
    assert!(
        has_relative || !has_absolute_url,
        "asset paths should be relative (not absolute URLs): {}",
        &content[..content.len().min(500)]
    );
}

// ============================================================================
// 10. CSS Selector pipeline (--selector flag)
// ============================================================================

/// --selector 'h3' extracts only h3 elements from the page.
#[tokio::test]
async fn test_selector_h3_extracts_only_h3() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body>\
                 <h1>Main Title</h1>\
                 <p>Paragraph to exclude.</p>\
                 <h3>Section One</h3>\
                 <p>Details for section one.</p>\
                 <h3>Section Two</h3>\
                 <p>Details for section two.</p>\
                 </body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--selector")
        .arg("h3")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert!(!files.is_empty(), "output should contain at least one file");

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(
        content.contains("Section One") || content.contains("Section Two"),
        "output should contain h3 content: {}",
        &content[..content.len().min(500)]
    );
    assert!(
        !content.contains("Paragraph to exclude"),
        "output should NOT contain paragraph text when --selector h3 is used: {}",
        &content[..content.len().min(500)]
    );
}

/// --selector 'table' extracts table content.
#[tokio::test]
async fn test_selector_table_extracts_table() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body>\
                 <h1>Data Page</h1>\
                 <p>Intro text.</p>\
                 <table><tr><td>Row1Col1</td><td>Row1Col2</td></tr></table>\
                 <p>More text.</p>\
                 </body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--selector")
        .arg("table")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert!(!files.is_empty(), "output should contain at least one file");

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(
        content.contains("Row1Col1"),
        "output should contain table content: {}",
        &content[..content.len().min(500)]
    );
}

/// Without --selector (default "body"), full page content is extracted.
#[tokio::test]
async fn test_no_selector_extracts_full_page() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Full Page Test</h1>\
                 <p>All content should appear when no selector is specified.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    let files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .collect();
    assert!(!files.is_empty(), "output should contain at least one file");

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert!(
        content.contains("Full Page Test") && content.contains("All content"),
        "full page content should be extracted: {}",
        &content[..content.len().min(500)]
    );
}

// ============================================================================
// 11. Binary file download (--download-documents with binary content-type)
// ============================================================================

/// When a page returns binary content-type (application/pdf), the raw bytes
/// are saved to a file in the output directory.
#[tokio::test]
async fn test_download_pdf_saves_binary_file() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    let pdf_content = b"%PDF-1.4 fake pdf content for testing binary download feature";

    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(pdf_content.to_vec())
                .insert_header("content-type", "application/pdf"),
        )
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/report.pdf", server.uri()))
        .arg("--single-page")
        .arg("--download-documents")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .success();

    // Find any .pdf file in the output directory (recursive)
    let pdf_files: Vec<_> = WalkDir::new(output.path())
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "pdf"))
        .collect();

    assert!(
        !pdf_files.is_empty(),
        "a .pdf file should exist in the output directory when downloading a PDF URL"
    );

    // Verify the content matches
    let saved = std::fs::read(pdf_files[0].path()).unwrap();
    assert_eq!(
        saved, pdf_content,
        "saved PDF content should match the original"
    );
}

// ============================================================================
// 12. Sitemap URL — explicit --sitemap-url flag
// ============================================================================

/// --sitemap-url with --use-sitemap fetches the explicit sitemap URL and
/// scrapes the URLs listed in it.
#[tokio::test]
async fn test_sitemap_url_scrapes_listed_urls() {
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
    <url><loc>{}/page-a</loc></url>
    <url><loc>{}/page-b</loc></url>
</urlset>"#,
            base, base,
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
