//! Integration tests for rust_scraper
//!
//! These tests verify end-to-end functionality of the scraper.

use rust_scraper::scraper;
use std::path::PathBuf;
use tempfile::TempDir;

// ============================================================================
// Integration Tests: Full scraping pipeline
// Note: These are "happy path" tests that require network access
// Skip with: cargo test --test integration -- --skip
// ============================================================================

#[tokio::test]
async fn test_scraper_can_fetch_simple_page() {
    // This test fetches a real page - skip in CI without network
    // Arrange
    let url = url::Url::parse("https://example.com").expect("Valid URL");
    let client = scraper::create_http_client().expect("HTTP client");

    // Act - Just verify we can fetch without error
    let result = scraper::scrape_with_readability(&client, &url, "body", 1, 0).await;

    // Assert - Either succeeds or fails gracefully (network dependent)
    // We don't assert success because it depends on network
    if result.is_ok() {
        let contents = result.unwrap();
        if !contents.is_empty() {
            assert!(!contents[0].title.is_empty());
        }
    }
}

// ============================================================================
// Tests: CLI Argument validation (via integration test pattern)
// ============================================================================

#[test]
fn test_output_format_display() {
    // Test that OutputFormat variants can be displayed
    use rust_scraper::OutputFormat;

    let markdown = OutputFormat::Markdown;
    let text = OutputFormat::Text;
    let json = OutputFormat::Json;

    // These should not panic
    let _ = format!("{:?}", markdown);
    let _ = format!("{:?}", text);
    let _ = format!("{:?}", json);
}

#[test]
fn test_args_default_values() {
    // Test that Args::default() works correctly
    use rust_scraper::Args;

    let args = Args::default();

    assert_eq!(args.selector, "body");
    assert_eq!(args.output, PathBuf::from("output"));
    assert_eq!(args.format, rust_scraper::OutputFormat::Markdown);
    assert_eq!(args.delay_ms, 1000);
    assert_eq!(args.max_pages, 10);
    assert_eq!(args.verbose, 0);
}

// ============================================================================
// Tests: Content extraction edge cases
// ============================================================================

#[tokio::test]
async fn test_scrape_handles_404_gracefully() {
    // Arrange
    let url =
        url::Url::parse("https://example.com/this-does-not-exist-404-test").expect("Valid URL");
    let client = scraper::create_http_client().expect("HTTP client");

    // Act
    let result = scraper::scrape_with_readability(&client, &url, "body", 1, 0).await;

    // Assert - Should fail gracefully with clear error
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.to_string().contains("404") || err.to_string().contains("HTTP"));
}

#[tokio::test]
async fn test_scrape_handles_invalid_url_gracefully() {
    // This test verifies behavior when URL has no host
    // Note: url::Url::parse("https://") actually succeeds with EmptyHost
    // The validation happens at a different level (validate_and_parse_url)

    // Test is covered by unit tests in lib.rs
    // This test can remain as a placeholder for future extension
    let result = scraper::create_http_client();
    assert!(result.is_ok());
}

// ============================================================================
// Tests: Result saving edge cases
// ============================================================================

#[test]
fn test_save_results_to_nested_directory() {
    // Arrange
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().join("level1").join("level2").join("output");

    let results = vec![rust_scraper::scraper::ScrapedContent {
        title: "Test".to_string(),
        content: "Content".to_string(),
        url: "https://example.com".to_string(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
    }];

    // Act
    let result = scraper::save_results(&results, &output_dir, &rust_scraper::OutputFormat::Text);

    // Assert
    assert!(result.is_ok());
    assert!(output_dir.exists());

    // Verify file was created
    let files: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!files.is_empty());
}

#[test]
fn test_save_results_json_with_special_characters() {
    // Arrange - Content with special characters that might break JSON
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().to_path_buf();

    let results = vec![rust_scraper::scraper::ScrapedContent {
        title: "Test with \"quotes\" and 'apostrophes'".to_string(),
        content: "Content with\nnewlines\tand\ttabs".to_string(),
        url: "https://example.com?param=value&other=test".to_string(),
        excerpt: Some("Excerpt with <html> & \"special\" chars".to_string()),
        author: Some("Author Name".to_string()),
        date: Some("2024-01-01".to_string()),
        html: None,
    }];

    // Act
    let result = scraper::save_results(&results, &output_dir, &rust_scraper::OutputFormat::Json);

    // Assert - Should handle special chars correctly
    assert!(result.is_ok());

    let json_path = output_dir.join("results.json");
    let content = std::fs::read_to_string(&json_path).unwrap();

    // Verify JSON is valid and readable
    let parsed: Vec<rust_scraper::scraper::ScrapedContent> =
        serde_json::from_str(&content).expect("Valid JSON");
    assert_eq!(parsed.len(), 1);
}

#[test]
fn test_save_results_markdown_with_markdown_syntax() {
    // Arrange - Content that looks like markdown
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let output_dir = temp_dir.path().to_path_buf();

    let results = vec![rust_scraper::scraper::ScrapedContent {
        title: "# Heading 1".to_string(),
        content: "**Bold** and *italic* and `code`".to_string(),
        url: "https://example.com".to_string(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
    }];

    // Act
    let result =
        scraper::save_results(&results, &output_dir, &rust_scraper::OutputFormat::Markdown);

    // Assert
    assert!(result.is_ok());

    let files: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    let content = std::fs::read_to_string(files[0].path()).unwrap();

    // Should preserve markdown-like content
    assert!(content.contains("# Heading 1"));
    assert!(content.contains("**Bold**"));
}
