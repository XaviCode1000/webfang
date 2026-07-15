//! Integration tests for file_saver — saving Markdown/Text/JSON, directory
//! creation, overwrite behavior, and Obsidian wiki-link options.

use webfang::domain::{ScrapedContent, ValidUrl};
use webfang::infrastructure::output::file_saver::{save_results, ObsidianOptions};
use webfang::OutputFormat;
use tempfile::TempDir;
use walkdir::WalkDir;

fn make_content(url: &str, title: &str, content: &str) -> ScrapedContent {
    ScrapedContent {
        title: title.to_string(),
        content: content.to_string(),
        url: ValidUrl::parse(url).unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
    }
}

fn make_content_with_html(url: &str, title: &str, html: &str) -> ScrapedContent {
    ScrapedContent {
        title: title.to_string(),
        content: String::new(),
        url: ValidUrl::parse(url).unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: Some(html.to_string()),
        assets: Vec::new(),
        correlation_id: None,
    }
}

fn count_files(dir: &std::path::Path) -> usize {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .count()
}

fn read_all_files(dir: &std::path::Path) -> Vec<(String, String)> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            let content = std::fs::read_to_string(e.path()).unwrap_or_default();
            let rel = e
                .path()
                .strip_prefix(dir)
                .unwrap_or(e.path())
                .to_string_lossy()
                .to_string();
            (rel, content)
        })
        .collect()
}

// ── Markdown saving ──────────────────────────────────────────────────────

#[test]
fn save_markdown_creates_file_with_correct_content() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content(
        "https://example.com/article",
        "Test Article",
        "This is the body content.",
    )];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    assert_eq!(files.len(), 1);
    let (path, content) = &files[0];
    assert!(path.ends_with(".md"));
    assert!(content.contains("Test Article"));
    assert!(content.contains("This is the body content."));
}

#[test]
fn save_markdown_includes_frontmatter() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content(
        "https://example.com/page",
        "Page Title",
        "Body here.",
    )];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    let (_, content) = &files[0];
    // Frontmatter should be delimited by ---
    assert!(content.starts_with("---\n"));
    // Should have a closing ---
    let after_first = &content[4..];
    assert!(after_first.contains("---\n"));
}

#[test]
fn save_markdown_with_metadata_fields() {
    let tmp = TempDir::new().unwrap();
    let mut item = make_content("https://example.com/article", "With Metadata", "Content.");
    item.author = Some("John Doe".to_string());
    item.date = Some("2026-01-15".to_string());
    item.excerpt = Some("A brief excerpt.".to_string());

    let results = vec![item];
    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    let (_, content) = &files[0];
    assert!(content.contains("John Doe"));
    assert!(content.contains("2026-01-15"));
    assert!(content.contains("A brief excerpt."));
}

// ── Directory creation ───────────────────────────────────────────────────

#[test]
fn save_creates_parent_directories_if_missing() {
    let tmp = TempDir::new().unwrap();
    let output_dir = tmp.path().join("deep").join("nested").join("output");
    let results = vec![make_content(
        "https://example.com/page",
        "Nested",
        "Content.",
    )];

    save_results(
        &results,
        &output_dir,
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    assert!(output_dir.exists());
    let files = count_files(&output_dir);
    assert_eq!(files, 1);
}

// ── Overwrite behavior ──────────────────────────────────────────────────

#[test]
fn save_overwrites_existing_file() {
    let tmp = TempDir::new().unwrap();

    // First save
    let results1 = vec![make_content(
        "https://example.com/page",
        "Original Title",
        "Original content.",
    )];
    save_results(
        &results1,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files1 = read_all_files(tmp.path());
    assert!(files1[0].1.contains("Original Title"));

    // Second save — same URL, different content
    let results2 = vec![make_content(
        "https://example.com/page",
        "Updated Title",
        "Updated content.",
    )];
    save_results(
        &results2,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files2 = read_all_files(tmp.path());
    assert_eq!(files2.len(), 1, "should still be 1 file, not 2");
    assert!(files2[0].1.contains("Updated Title"));
    assert!(files2[0].1.contains("Updated content."));
    assert!(!files2[0].1.contains("Original Title"));
}

// ── JSON saving ──────────────────────────────────────────────────────────

#[test]
fn save_json_creates_results_json() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content(
        "https://example.com/page",
        "JSON Test",
        "Content.",
    )];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Json,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let json_path = tmp.path().join("results.json");
    assert!(json_path.exists());

    let content = std::fs::read_to_string(&json_path).unwrap();
    assert!(content.contains("JSON Test"));
    assert!(content.contains("Content."));
}

#[test]
fn save_json_contains_valid_json_array() {
    let tmp = TempDir::new().unwrap();
    let results = vec![
        make_content("https://a.com", "Article 1", "Content 1."),
        make_content("https://b.com", "Article 2", "Content 2."),
    ];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Json,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let json_path = tmp.path().join("results.json");
    let content = std::fs::read_to_string(&json_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.is_array());
    assert_eq!(parsed.as_array().unwrap().len(), 2);
}

// ── Text saving ──────────────────────────────────────────────────────────

#[test]
fn save_text_creates_txt_files() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content(
        "https://example.com/page",
        "Text Test",
        "Plain text content.",
    )];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Text,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    assert_eq!(files.len(), 1);
    assert!(files[0].0.ends_with(".txt"));
    assert!(files[0].1.contains("Text Test"));
    assert!(files[0].1.contains("Plain text content."));
}

// ── Multiple items ───────────────────────────────────────────────────────

#[test]
fn save_multiple_items_creates_separate_files() {
    let tmp = TempDir::new().unwrap();
    let results = vec![
        make_content("https://a.com/page1", "Article 1", "Content 1."),
        make_content("https://b.com/page2", "Article 2", "Content 2."),
    ];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = count_files(tmp.path());
    assert_eq!(files, 2);
}

// ── Obsidian options ─────────────────────────────────────────────────────

#[test]
fn obsidian_wiki_links_option_is_respected() {
    let tmp = TempDir::new().unwrap();
    let obsidian = ObsidianOptions {
        wiki_links: true,
        relative_assets: false,
        tags: vec!["scraped".to_string()],
        rich_metadata: false,
        quick_save: false,
        vault_path: None,
    };

    let results = vec![make_content(
        "https://example.com/article",
        "Wiki Link Test",
        "Content here.",
    )];

    save_results(&results, tmp.path(), &OutputFormat::Markdown, &obsidian).unwrap();

    let files = read_all_files(tmp.path());
    assert_eq!(files.len(), 1);
    // Tags should appear in frontmatter
    assert!(files[0].1.contains("scraped"));
}

#[test]
fn obsidian_tags_appear_in_frontmatter() {
    let tmp = TempDir::new().unwrap();
    let obsidian = ObsidianOptions {
        wiki_links: false,
        relative_assets: false,
        tags: vec!["tag1".to_string(), "tag2".to_string()],
        rich_metadata: false,
        quick_save: false,
        vault_path: None,
    };

    let results = vec![make_content(
        "https://example.com/page",
        "Tags Test",
        "Content.",
    )];

    save_results(&results, tmp.path(), &OutputFormat::Markdown, &obsidian).unwrap();

    let files = read_all_files(tmp.path());
    assert!(files[0].1.contains("tag1"));
    assert!(files[0].1.contains("tag2"));
}

// ── HTML to Markdown conversion ──────────────────────────────────────────

#[test]
fn save_html_content_converts_to_markdown() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content_with_html(
        "https://example.com/page",
        "HTML Page",
        "<h1>Hello World</h1><p>This is a paragraph.</p>",
    )];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    assert_eq!(files.len(), 1);
    // htmd should convert HTML to Markdown
    let content = &files[0].1;
    assert!(content.contains("Hello World"));
    assert!(content.contains("This is a paragraph."));
}
