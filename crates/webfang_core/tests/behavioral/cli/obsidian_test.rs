//! Obsidian-specific behavior: wiki-links, tags, quick-save.

use crate::BehavioralTest;
use std::path::Path;
use walkdir::WalkDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Snapshot Obsidian markdown output with deterministic redactions.
///
/// `crate::redact_nondeterministic` collapses the temp-dir path, ANSI codes,
/// dynamic wiremock ports, and ISO-8601 `-Z` timestamps. The frontmatter
/// `date:` field is a bare `YYYY-MM-DD` (no time) and `scrape_date:` is
/// `YYYY-MM-DDThh:mm:ss+0000`; neither is caught by that helper. insta's
/// `add_filter` applies a regex onto the final snapshot string (the correct
/// insta-native mechanism for free-text snapshots — `redactions` uses path
/// selectors and cannot match raw lines), collapsing those fields to stable
/// markers before snapshotting.
fn assert_obsidian_snapshot(name: &str, dir: &Path, content: &str) {
    let redacted = crate::redact_nondeterministic(dir, content);
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"date: \d{4}-\d{2}-\d{2}", "date: [DATE]");
    settings.add_filter(
        r"scrape_date: \d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}[+-]\d{4}",
        "scrape_date: [SCRAPE_DATE]",
    );
    settings.bind(|| {
        insta::assert_snapshot!(name, redacted);
    });
}

const PAGE_WITH_LINKS: &str = r#"
<html><head><title>Wiki Links Test</title></head>
<body><article>
<h1>Wiki Links Test</h1>
<p>Check out <a href="/other-page">this other page</a> for more info.
Also see <a href="/third-page">the third page</a>.</p>
</article></body></html>
"#;

const TAGGED_PAGE: &str = r#"
<html><head><title>Tagged Page</title></head>
<body><article>
<h1>Tagged Page</h1>
<p>Content with obsidian tags for frontmatter testing.</p>
</article></body></html>
"#;

// ---------------------------------------------------------------------------
// --obsidian-wiki-links
// ---------------------------------------------------------------------------

#[tokio::test]
async fn obsidian_wiki_links_produces_wiki_syntax() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_LINKS))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--obsidian-wiki-links")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    assert_obsidian_snapshot(
        "obsidian_wiki_links_produces_wiki_syntax",
        t.out.path(),
        &content,
    );
}

#[tokio::test]
async fn obsidian_wiki_links_removes_absolute_urls() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_LINKS))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--obsidian-wiki-links")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    assert_obsidian_snapshot(
        "obsidian_wiki_links_removes_absolute_urls",
        t.out.path(),
        &content,
    );
}

// ---------------------------------------------------------------------------
// --obsidian-tags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn obsidian_tags_appear_in_frontmatter() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TAGGED_PAGE))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--obsidian-tags")
        .arg("scraped,web-dev,rust")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    assert_obsidian_snapshot(
        "obsidian_tags_appear_in_frontmatter",
        t.out.path(),
        &content,
    );
}

#[tokio::test]
async fn obsidian_tags_produces_yaml_frontmatter() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TAGGED_PAGE))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--obsidian-tags")
        .arg("test")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    assert_obsidian_snapshot(
        "obsidian_tags_produces_yaml_frontmatter",
        t.out.path(),
        &content,
    );
}

// ---------------------------------------------------------------------------
// --quick-save
// ---------------------------------------------------------------------------

#[tokio::test]
async fn quick_save_creates_files_in_inbox() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TAGGED_PAGE))
        .expect(1)
        .mount(&t.server)
        .await;

    // --quick-save requires --vault to determine where _inbox lives
    // Create a mock vault structure in the output dir
    let vault_dir = t.out.path().join("test_vault");
    std::fs::create_dir_all(vault_dir.join(".obsidian")).unwrap();
    std::fs::write(
        vault_dir.join(".obsidian").join("obsidian.json"),
        r#"{"vault":{"fsPath":"/tmp/test","id":"test","name":"Test"}}"#,
    )
    .unwrap();

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--quick-save")
        .arg("--vault")
        .arg(&vault_dir)
        .arg("--quiet")
        .assert()
        .success();

    // Check that files ended up in _inbox somewhere under the vault
    let has_inbox = WalkDir::new(&vault_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| e.path().to_string_lossy().contains("_inbox"));
    assert!(
        has_inbox,
        "--quick-save should place files in _inbox directory"
    );
}
