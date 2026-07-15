//! Integration tests for file_saver — directory creation, markdown writing,
//! overwrite behavior, and content verification.

use webfang::domain::{ScrapedContent, ValidUrl};
use webfang::infrastructure::output::file_saver::{save_results, ObsidianOptions};
use webfang::OutputFormat;
use std::fs;
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

fn make_html_content(url: &str, title: &str, html: &str) -> ScrapedContent {
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

// ── Directory creation ────────────────────────────────────────────────────

#[test]
fn creates_output_directory_if_missing() {
    let tmp = TempDir::new().unwrap();
    let output = tmp.path().join("nested").join("output");

    let results = vec![make_content("https://example.com/p", "Title", "Body")];
    save_results(
        &results,
        &output,
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    assert!(output.exists());
    assert_eq!(count_files(&output), 1);
}

#[test]
fn creates_multiple_parent_levels() {
    let tmp = TempDir::new().unwrap();
    let output = tmp.path().join("a").join("b").join("c").join("d");

    let results = vec![make_content("https://example.com/p", "Deep", "Content")];
    save_results(
        &results,
        &output,
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    assert!(output.exists());
}

// ── Markdown writing ──────────────────────────────────────────────────────

#[test]
fn markdown_file_contains_title_and_content() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content(
        "https://example.com/article",
        "Test Article",
        "This is the body.",
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
    assert!(files[0].0.ends_with(".md"));
    assert!(files[0].1.contains("Test Article"));
    assert!(files[0].1.contains("This is the body."));
}

#[test]
fn markdown_includes_frontmatter_delimiters() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content("https://example.com/p", "Title", "Body")];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    assert!(files[0].1.starts_with("---\n"));
    let after_first = &files[0].1[4..];
    assert!(after_first.contains("---\n"));
}

#[test]
fn markdown_with_metadata_fields() {
    let tmp = TempDir::new().unwrap();
    let mut item = make_content("https://example.com/p", "With Meta", "Content");
    item.author = Some("Jane Doe".to_string());
    item.date = Some("2025-06-01".to_string());
    item.excerpt = Some("A short excerpt.".to_string());

    save_results(
        &[item],
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    assert!(files[0].1.contains("Jane Doe"));
    assert!(files[0].1.contains("2025-06-01"));
    assert!(files[0].1.contains("A short excerpt."));
}

#[test]
fn html_content_converts_to_markdown() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_html_content(
        "https://example.com/p",
        "HTML Page",
        "<h1>Hello</h1><p>Paragraph text.</p>",
    )];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    assert!(files[0].1.contains("Hello"));
    assert!(files[0].1.contains("Paragraph text."));
}

// ── Overwrite behavior ───────────────────────────────────────────────────

#[test]
fn second_save_overwrites_first_for_same_url() {
    let tmp = TempDir::new().unwrap();

    save_results(
        &[make_content(
            "https://example.com/p",
            "Original",
            "Original content.",
        )],
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    save_results(
        &[make_content(
            "https://example.com/p",
            "Updated",
            "Updated content.",
        )],
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let files = read_all_files(tmp.path());
    assert_eq!(files.len(), 1, "should still be 1 file");
    assert!(files[0].1.contains("Updated"));
    assert!(!files[0].1.contains("Original"));
}

// ── JSON output ───────────────────────────────────────────────────────────

#[test]
fn json_output_creates_results_json() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content("https://example.com/p", "JSON Test", "Body")];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Json,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let path = tmp.path().join("results.json");
    assert!(path.exists());

    let content = fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.is_array());
    assert_eq!(parsed.as_array().unwrap().len(), 1);
}

#[test]
fn json_output_contains_valid_json_for_multiple_items() {
    let tmp = TempDir::new().unwrap();
    let results = vec![
        make_content("https://a.com", "A", "Content A"),
        make_content("https://b.com", "B", "Content B"),
        make_content("https://c.com", "C", "Content C"),
    ];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Json,
        &ObsidianOptions::default(),
    )
    .unwrap();

    let content = fs::read_to_string(tmp.path().join("results.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 3);
}

// ── Text output ───────────────────────────────────────────────────────────

#[test]
fn text_output_creates_txt_file() {
    let tmp = TempDir::new().unwrap();
    let results = vec![make_content(
        "https://example.com/p",
        "Text Test",
        "Plain text.",
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
    assert!(files[0].1.contains("Plain text."));
}

// ── Multiple items ────────────────────────────────────────────────────────

#[test]
fn multiple_items_create_separate_files() {
    let tmp = TempDir::new().unwrap();
    let results = vec![
        make_content("https://a.com/p1", "Article 1", "Content 1"),
        make_content("https://b.com/p2", "Article 2", "Content 2"),
    ];

    save_results(
        &results,
        tmp.path(),
        &OutputFormat::Markdown,
        &ObsidianOptions::default(),
    )
    .unwrap();

    assert_eq!(count_files(tmp.path()), 2);
}

// ── Obsidian options ──────────────────────────────────────────────────────

#[test]
fn obsidian_tags_appear_in_frontmatter() {
    let tmp = TempDir::new().unwrap();
    let obsidian = ObsidianOptions {
        wiki_links: false,
        relative_assets: false,
        tags: vec!["scraped".to_string(), "rust".to_string()],
        rich_metadata: false,
        quick_save: false,
        vault_path: None,
    };

    let results = vec![make_content("https://example.com/p", "Tagged", "Body")];
    save_results(&results, tmp.path(), &OutputFormat::Markdown, &obsidian).unwrap();

    let files = read_all_files(tmp.path());
    assert!(files[0].1.contains("scraped"));
    assert!(files[0].1.contains("rust"));
}
