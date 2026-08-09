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

// ---------------------------------------------------------------------------
// #638: --quick-save + --download-images must keep the vault self-contained.
// Regression: the Downloader was wiring assets to `-o` (via ScraperConfig)
// while the Markdown went to the vault, so relative paths escaped the vault.
// ---------------------------------------------------------------------------

// Small 1x1 PNG (valid PNG header).
const PNG_BYTES: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x01, 0xE2, 0x21, 0xBC, 0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E,
    0x44, 0xAE, 0x42, 0x60, 0x82,
];

const PAGE_WITH_IMG: &str = r#"
<html><body><article>
<h1>Obsidian Vault Image Test</h1>
<p>Embedded image that must land inside the vault.</p>
<img src="/photo.png" alt="vault photo">
</article></body></html>
"#;

#[tokio::test]
async fn quick_save_download_images_stay_inside_vault() {
    let server = wiremock::MockServer::start().await;
    // `-o` is deliberately distinct from the vault, reproducing the #638 setup.
    let cli_out = tempfile::TempDir::new().unwrap();
    let vault_dir = cli_out.path().join("vault");
    std::fs::create_dir_all(vault_dir.join(".obsidian")).unwrap();
    std::fs::write(
        vault_dir.join(".obsidian").join("obsidian.json"),
        r#"{"vault":{"fsPath":"/tmp/test","id":"test","name":"Test"}}"#,
    )
    .unwrap();

    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_WITH_IMG))
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/photo.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(PNG_BYTES.to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    crate::cmd()
        .arg("--url")
        .arg(format!("{}/page", server.uri()))
        .arg("--single-page")
        .arg("--format")
        .arg("markdown")
        .arg("--output")
        .arg(cli_out.path())
        .arg("--quick-save")
        .arg("--vault")
        .arg(&vault_dir)
        .arg("--download-images")
        .arg("--obsidian-relative-assets")
        .arg("--quiet")
        .assert()
        .success();

    // 1) Images must land INSIDE the vault (self-contained output).
    let vault_images = vault_dir.join("images");
    assert!(
        vault_images.exists(),
        "downloaded images should live inside the vault, but {vault_images:?} is missing"
    );
    let image_files: Vec<_> = std::fs::read_dir(&vault_images)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_ok_and(|ft| ft.is_file()))
        .collect();
    assert!(
        !image_files.is_empty(),
        "at least one image should be saved inside the vault"
    );

    // 2) Regression: `-o` must NOT receive the downloaded assets anymore.
    let cli_out_images = cli_out.path().join("images");
    assert!(
        !cli_out_images.exists(),
        "-o/images must not contain assets when --quick-save targets a vault (#638)"
    );

    // 3) Semantic: every relative asset reference written into the vault's
    // Markdown must resolve to a path INSIDE the vault (self-contained).
    let vault_canon = vault_dir.canonicalize().unwrap();
    for entry in WalkDir::new(&vault_dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|e| e.to_str()) != Some("md")
        {
            continue;
        }
        let md_dir = entry.path().parent().unwrap();
        let content = std::fs::read_to_string(entry.path()).unwrap();
        // Find Markdown image references: ![alt](target)
        for captures in content.match_indices("](") {
            let after = &content[captures.0 + 2..];
            let end = after.find(')').unwrap_or(after.len());
            let target = &after[..end];
            // Skip remote/absolute URLs and wiki-links.
            if target.contains("://") || target.starts_with('#') || target.contains("[[") {
                continue;
            }
            let resolved = md_dir.join(target);
            let canonical = resolved.canonicalize().unwrap_or_else(|_| {
                panic!(
                    "Obsidian asset ref `{target}` in {} does not resolve to an existing file",
                    entry.path().display()
                )
            });
            assert!(
                canonical.starts_with(&vault_canon),
                "asset ref `{target}` in {} escapes the vault: {}",
                entry.path().display(),
                canonical.display()
            );
        }
    }
}
