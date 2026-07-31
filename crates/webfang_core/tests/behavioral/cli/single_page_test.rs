//! Single-page scraping: format variants, CSS selectors, and quiet mode.

use crate::assert_snapshot_redacted;
use crate::BehavioralTest;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SEED_HTML: &str = r#"
<html><head><title>Single Page Test</title></head>
<body><main><article>
<h1>Hello World</h1>
<p>This is meaningful content for the extractor to process.</p>
<p>More paragraphs ensure readability can extract a proper document.</p>
</article></main></body></html>
"#;

// ---------------------------------------------------------------------------
// Markdown output (default)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_page_creates_md_file() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    crate::assert_snapshot_redacted("single_page_creates_md_file", t.out.path(), content);
}

#[tokio::test]
async fn single_page_md_contains_page_content() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    crate::assert_snapshot_redacted(
        "single_page_md_contains_page_content",
        t.out.path(),
        content,
    );
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_page_json_format_creates_json_file() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("json")
        .arg("--quiet")
        .assert()
        .success();

    // JSON output goes to results.json at the output root (not in domain subdirs)
    let json_files = t.find_files("json");
    let content = std::fs::read_to_string(&json_files[0]).expect("read .json output");
    crate::assert_snapshot_redacted(
        "single_page_json_format_creates_json_file",
        t.out.path(),
        content,
    );
}

#[tokio::test]
async fn single_page_json_has_correct_structure() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("json")
        .arg("--quiet")
        .assert()
        .success();

    let json_files = t.find_files("json");
    let content = std::fs::read_to_string(&json_files[0]).expect("read .json output");
    crate::assert_snapshot_redacted(
        "single_page_json_has_correct_structure",
        t.out.path(),
        content,
    );
}

// ---------------------------------------------------------------------------
// Text output
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_page_text_format_creates_txt_file() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("text")
        .arg("--quiet")
        .assert()
        .success();

    let txt_files = t.find_files("txt");
    let content = std::fs::read_to_string(&txt_files[0]).expect("read .txt output");
    crate::assert_snapshot_redacted(
        "single_page_text_format_creates_txt_file",
        t.out.path(),
        content,
    );
}

#[tokio::test]
async fn single_page_text_is_plain_text() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("text")
        .arg("--quiet")
        .assert()
        .success();

    let txt_files = t.find_files("txt");
    let content = std::fs::read_to_string(&txt_files[0]).expect("read .txt output");
    crate::assert_snapshot_redacted("single_page_text_is_plain_text", t.out.path(), content);
}

// ---------------------------------------------------------------------------
// Quiet mode
// ---------------------------------------------------------------------------

#[tokio::test]
async fn single_page_quiet_suppresses_stdout() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    let output = t
        .scraper_cmd()
        .arg("--single-page")
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert!(
        output.status.success(),
        "expected success, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // stdout should be empty or very short in quiet mode; snapshot it to lock the
    // behavior so a regression (banner re-added, stderr leaking into stdout) fails.
    let stdout = String::from_utf8_lossy(&output.stdout);
    crate::assert_snapshot_plain("single_page_quiet_suppresses_stdout", stdout);
}

// ---------------------------------------------------------------------------
// CSS Selector pipeline (--selector flag)
// ---------------------------------------------------------------------------

/// --selector 'h3' extracts only h3 elements from the page.
#[tokio::test]
async fn selector_h3_extracts_only_h3() {
    let server = MockServer::start().await;
    let output = tempfile::TempDir::new().unwrap();

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

    crate::cmd()
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
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();
    assert!(
        !files.is_empty(),
        "output should contain at least one .md file"
    );

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert_snapshot_redacted("selector_h3_extracts_only_h3", output.path(), &content);
}

/// --selector 'table' extracts table content.
#[tokio::test]
async fn selector_table_extracts_table() {
    let server = MockServer::start().await;
    let output = tempfile::TempDir::new().unwrap();

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

    crate::cmd()
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
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();
    assert!(
        !files.is_empty(),
        "output should contain at least one .md file"
    );

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert_snapshot_redacted("selector_table_extracts_table", output.path(), &content);
}

/// Without --selector (default "body"), full page content is extracted.
#[tokio::test]
async fn no_selector_extracts_full_page() {
    let server = MockServer::start().await;
    let output = tempfile::TempDir::new().unwrap();

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

    crate::cmd()
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
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .collect();
    assert!(
        !files.is_empty(),
        "output should contain at least one .md file"
    );

    let content = std::fs::read_to_string(files[0].path()).unwrap();
    assert_snapshot_redacted("no_selector_extracts_full_page", output.path(), &content);
}
