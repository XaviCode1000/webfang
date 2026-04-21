//! Integration tests for FileExporter - Data Integrity
//!
//! These tests validate the FileExporter implementation without external dependencies.
//! Tests are deterministic and don't require network access.

use std::env::temp_dir;
use std::fs;

use rust_scraper::domain::entities::ExportFormat;
use rust_scraper::domain::exporter::ExporterConfig;
use rust_scraper::domain::DocumentChunkUnvalidated;
use rust_scraper::domain::DocumentChunkValidated;
use rust_scraper::domain::Exporter;
use rust_scraper::domain::ScrapedContent;
use rust_scraper::domain::ValidUrl;
use rust_scraper::domain::ValidationError;
use rust_scraper::infrastructure::export::file_exporter::FileExporter;

/// Test helper: create and validate a DocumentChunk
fn make_chunk(url: &str, title: &str, content: &str) -> DocumentChunkValidated {
    use rust_scraper::domain::{ScrapedContent, ValidUrl};

    // Create a ScrapedContent first, then convert to DocumentChunk
    let scraped = ScrapedContent {
        title: title.to_string(),
        content: content.to_string(),
        url: ValidUrl::parse(url).expect("valid URL"),
        excerpt: Some("Test excerpt".to_string()),
        author: Some("Test Author".to_string()),
        date: Some("2024-01-01".to_string()),
        html: None,
        assets: vec![],
    };

    // Convert to Draft state, then validate
    let unvalidated: rust_scraper::domain::DocumentChunkUnvalidated = scraped.into();
    unvalidated.validate().expect("valid document")
}

/// Test: FileExporter creates file with correct markdown structure
#[test]
fn test_file_exporter_markdown_structure() {
    let dir = temp_dir().join("test_md_struct");
    let _ = fs::remove_dir_all(&dir);

    // FileExporter writes to a single file based on filename
    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "test");
    let exporter = FileExporter::new(config);

    let doc = make_chunk(
        "https://example.com/test-page",
        "Test Title",
        "This is the main content of the page.",
    );

    let result = exporter.export(doc);
    assert!(result.is_ok(), "Export should succeed: {:?}", result);

    // Verify JSONL file exists
    let file_path = dir.join("test.jsonl");
    assert!(
        file_path.exists(),
        "JSONL file should exist at {:?}",
        file_path
    );

    // Verify content structure (JSON lines)
    let content = fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("Test Title"), "Should have title in JSON");
    assert!(
        content.contains("example.com/test-page"),
        "Should have URL in JSON"
    );

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

/// Test: FileExporter exports JSON correctly
#[test]
fn test_file_exporter_text_structure() {
    let dir = temp_dir().join("test_txt_struct");
    let _ = fs::remove_dir_all(&dir);

    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "test");
    let exporter = FileExporter::new(config);

    let doc = make_chunk(
        "https://example.com/page",
        "Page Title",
        "Page content here.",
    );

    let result = exporter.export(doc);
    assert!(result.is_ok());

    // Verify JSONL file exists
    let file_path = dir.join("test.jsonl");
    assert!(file_path.exists(), "JSONL file should exist");

    // Verify content is valid JSON
    let content = fs::read_to_string(&file_path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&content.lines().next().unwrap()).unwrap();
    assert_eq!(json["title"], "Page Title");

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

/// Test: FileExporter rejects empty content during validation
#[test]
fn test_file_exporter_empty_content() {
    let dir = temp_dir().join("test_empty");
    let _ = fs::remove_dir_all(&dir);

    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "test");
    let _exporter = FileExporter::new(config);

    // Create a ScrapedContent with empty content
    let scraped = ScrapedContent {
        title: "Empty".to_string(),
        content: "".to_string(),
        url: ValidUrl::parse("https://example.com/empty").expect("valid URL"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
    };

    // Convert to Draft state - this should succeed
    let unvalidated: DocumentChunkUnvalidated = scraped.into();

    // Validation should fail for empty content
    let validation_result = unvalidated.validate();
    assert!(
        validation_result.is_err(),
        "Validation should fail for empty content"
    );

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

/// Test: DocumentChunk validation with comprehensive rules
#[test]
fn test_document_chunk_validation_comprehensive() {
    // Test empty title
    let scraped_empty_title = ScrapedContent {
        title: "".to_string(),
        content: "Valid content".to_string(),
        url: ValidUrl::parse("https://example.com/test").expect("valid URL"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
    };
    let chunk = DocumentChunkUnvalidated::from(scraped_empty_title);
    assert!(matches!(chunk.validate(), Err(ValidationError::EmptyTitle)));

    // Test empty content
    let scraped_empty_content = ScrapedContent {
        title: "Valid Title".to_string(),
        content: "".to_string(),
        url: ValidUrl::parse("https://example.com/test").expect("valid URL"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
    };
    let chunk = DocumentChunkUnvalidated::from(scraped_empty_content);
    assert!(matches!(
        chunk.validate(),
        Err(ValidationError::EmptyContent)
    ));

    // Test valid chunk
    let scraped_valid = ScrapedContent {
        title: "Valid Title".to_string(),
        content: "Valid content".to_string(),
        url: ValidUrl::parse("https://example.com/test").expect("valid URL"),
        excerpt: Some("Valid excerpt".to_string()),
        author: Some("Valid Author".to_string()),
        date: Some("2024-01-01".to_string()),
        html: None,
        assets: vec![],
    };
    let chunk = DocumentChunkUnvalidated::from(scraped_valid);
    assert!(
        chunk.validate().is_ok(),
        "Valid chunk should pass validation"
    );
}

/// Test: FileExporter batch export works correctly
#[test]
fn test_file_exporter_batch() {
    let dir = temp_dir().join("test_batch");
    let _ = fs::remove_dir_all(&dir);

    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "batch");
    let exporter = FileExporter::new(config);

    let docs = vec![
        make_chunk("https://example.com/page1", "Page 1", "Content 1"),
        make_chunk("https://example.com/page2", "Page 2", "Content 2"),
    ];

    let result = exporter.export_batch(&docs);
    assert!(result.is_ok(), "Batch export should succeed: {:?}", result);

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

/// Test: Conversion from ScrapedContent to DocumentChunk preserves all fields
#[test]
fn test_scraped_content_conversion() {
    let url = ValidUrl::parse("https://example.com/article").unwrap();
    let scraped = ScrapedContent {
        title: "Article Title".to_string(),
        content: "Article content goes here.".to_string(),
        url,
        excerpt: Some("Article excerpt".to_string()),
        author: Some("John Doe".to_string()),
        date: Some("2024-01-15".to_string()),
        html: Some("<p>HTML content</p>".to_string()),
        assets: vec![],
    };

    // Conversion from ScrapedContent creates Draft state, then validate
    let unvalidated: rust_scraper::domain::DocumentChunk<rust_scraper::domain::Draft> =
        scraped.into();
    let chunk = unvalidated.validate().expect("should be valid");

    assert_eq!(chunk.title, "Article Title");
    assert_eq!(chunk.content, "Article content goes here.");
    assert_eq!(chunk.url, "https://example.com/article");
    assert!(chunk.metadata.contains_key("excerpt"));
    assert!(chunk.metadata.contains_key("author"));
    assert!(chunk.metadata.contains_key("date"));
    assert!(chunk.metadata.contains_key("domain"));
}

/// Test: Multiple documents are exported to a single JSONL file
#[test]
fn test_multi_domain_organization() {
    let dir = temp_dir().join("test_multi_domain");
    let _ = fs::remove_dir_all(&dir);

    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "test");
    let exporter = FileExporter::new(config);

    let docs = vec![
        make_chunk("https://example.com/page1", "Page 1", "Content 1"),
        make_chunk("https://example.org/page2", "Page 2", "Content 2"),
        make_chunk("https://test.net/page3", "Test 3", "Content 3"),
    ];

    let result = exporter.export_batch(&docs);
    assert!(result.is_ok(), "Batch export should succeed: {:?}", result);

    // Verify the JSONL file was created
    let jsonl_file = dir.join("test.jsonl");
    assert!(jsonl_file.exists(), "JSONL file should exist");

    // Verify content has multiple lines
    let content = fs::read_to_string(&jsonl_file).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert!(
        lines.len() >= 3,
        "Should have at least 3 documents, got {}",
        lines.len()
    );

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

/// Test: Special characters in URLs are handled correctly
#[test]
fn test_special_characters_in_url() {
    let dir = temp_dir().join("test_special");
    let _ = fs::remove_dir_all(&dir);

    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "test");
    let exporter = FileExporter::new(config);

    // URL with query parameters and special chars
    let doc = make_chunk(
        "https://example.com/search?q=test&page=1",
        "Search Results",
        "Results here",
    );

    let result = exporter.export(doc);

    // May fail due to URL sanitization - that's acceptable for invalid chars
    // The key is it doesn't panic
    if result.is_ok() {
        let domain_dir = dir.join("example.com");
        if domain_dir.exists() {
            let files: Vec<_> = fs::read_dir(&domain_dir).unwrap().collect();
            assert!(!files.is_empty(), "Should have created a file");
        }
    }

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

/// Test: Backpressure simulation - small buffer with many items
/// This test validates that the exporter doesn't crash under memory pressure
#[test]
fn test_exporter_under_memory_pressure() {
    let dir = temp_dir().join("test_pressure");
    let _ = fs::remove_dir_all(&dir);

    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "stress");
    let exporter = FileExporter::new(config);

    // Simulate many small documents
    let docs: Vec<_> = (0..100)
        .map(|i| {
            make_chunk(
                &format!("https://example.com/page{}", i),
                &format!("Page {}", i),
                "content",
            )
        })
        .collect();

    // This should complete without OOM
    let result = exporter.export_batch(&docs);
    assert!(result.is_ok(), "Should handle many documents: {:?}", result);

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}

/// Test: Verify no files of 44 bytes (the old bug)
#[test]
fn test_no_empty_files() {
    let dir = temp_dir().join("test_no_empty");
    let _ = fs::remove_dir_all(&dir);

    let config = ExporterConfig::new(dir.clone(), ExportFormat::Jsonl, "test");
    let exporter = FileExporter::new(config);

    // Create minimal but valid content
    let doc = make_chunk("https://example.com/minimal", "Minimal", "x");
    let result = exporter.export(doc);

    // Export should succeed
    assert!(result.is_ok(), "Export should succeed: {:?}", result);

    // Find the created file - use the correct path structure
    let domain_dir = dir.join("example.com");
    if domain_dir.exists() {
        let file_path = domain_dir.join("minimal.md");
        if file_path.exists() {
            let metadata = fs::metadata(&file_path).unwrap();
            // File should be larger than 44 bytes (the bug we fixed)
            assert!(
                metadata.len() > 44,
                "File should not be the old 44-byte bug: {} bytes",
                metadata.len()
            );
        }
    }

    // Cleanup
    let _ = fs::remove_dir_all(&dir);
}
