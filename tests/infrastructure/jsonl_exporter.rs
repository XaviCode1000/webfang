//! Integration tests for JsonlExporter — file creation, append, concurrent writes.
//!
//! Uses tempfile for real I/O verification of JSONL output.

use webfang::domain::entities::ExportFormat;
use webfang::domain::exporter::{Exporter, ExporterConfig};
use webfang::domain::{DocumentChunkUnvalidated, ScrapedContent, ValidUrl};
use webfang::infrastructure::export::JsonlExporter;
use std::fs;
use tempfile::TempDir;

fn make_chunk(title: &str) -> webfang::domain::DocumentChunkValidated {
    let scraped = ScrapedContent {
        title: title.to_string(),
        content: format!("Content for {title}"),
        url: ValidUrl::parse("https://example.com/test").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
    };
    let unvalidated: DocumentChunkUnvalidated = scraped.into();
    unvalidated.validate().expect("valid document")
}

// ── File creation ─────────────────────────────────────────────────────────

#[test]
fn creates_jsonl_file_with_single_document() {
    let tmp = TempDir::new().unwrap();
    let config = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "output");
    let exporter = JsonlExporter::new(config);

    exporter.export(make_chunk("Hello")).unwrap();

    let path = tmp.path().join("output.jsonl");
    assert!(path.exists(), "JSONL file should be created");

    let content = fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 1);

    let json: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
    assert_eq!(json["title"], "Hello");
    assert_eq!(json["content"], "Content for Hello");
}

#[test]
fn creates_parent_directories_if_missing() {
    let tmp = TempDir::new().unwrap();
    let deep = tmp.path().join("a").join("b").join("c");
    let config = ExporterConfig::new(deep.clone(), ExportFormat::Jsonl, "out");
    let exporter = JsonlExporter::new(config);

    exporter.export(make_chunk("Deep")).unwrap();

    assert!(deep.join("out.jsonl").exists());
}

// ── Append behavior ───────────────────────────────────────────────────────

#[test]
fn appends_to_existing_file_without_overwriting() {
    let tmp = TempDir::new().unwrap();

    // First write
    let config1 = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "test");
    let exporter1 = JsonlExporter::new(config1);
    exporter1.export(make_chunk("First")).unwrap();

    // Second write (append mode)
    let config2 = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "test");
    let exporter2 = JsonlExporter::new(config2);
    exporter2.export(make_chunk("Second")).unwrap();

    let content = fs::read_to_string(tmp.path().join("test.jsonl")).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "should have 2 lines after two exports");
    assert!(content.contains("First"));
    assert!(content.contains("Second"));
}

#[test]
fn batch_export_creates_all_lines() {
    let tmp = TempDir::new().unwrap();
    let config = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "batch");
    let exporter = JsonlExporter::new(config);

    let chunks: Vec<_> = (0..5).map(|i| make_chunk(&format!("Doc {i}"))).collect();
    exporter.export_batch(&chunks).unwrap();

    let content = fs::read_to_string(tmp.path().join("batch.jsonl")).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 5);

    // Each line should be valid JSON
    for line in &lines {
        let json: serde_json::Value = serde_json::from_str(line).unwrap();
        assert!(json.is_object());
    }
}

// ── File content verification ─────────────────────────────────────────────

#[test]
fn each_line_is_valid_json() {
    let tmp = TempDir::new().unwrap();
    let config = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "valid");
    let exporter = JsonlExporter::new(config);

    for i in 0..10 {
        exporter.export(make_chunk(&format!("Item {i}"))).unwrap();
    }

    let content = fs::read_to_string(tmp.path().join("valid.jsonl")).unwrap();
    for (i, line) in content.lines().enumerate() {
        let json: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {i} is not valid JSON: {e}"));
        assert!(json["title"].as_str().unwrap().contains("Item"));
    }
}

#[test]
fn jsonl_file_is_not_truncated_on_reopen() {
    let tmp = TempDir::new().unwrap();

    // Write 3 documents
    let config = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "persist");
    let exporter = JsonlExporter::new(config);
    for i in 0..3 {
        exporter.export(make_chunk(&format!("Doc {i}"))).unwrap();
    }

    // Create a NEW exporter pointing to same file (simulates restart)
    let config2 = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "persist");
    let exporter2 = JsonlExporter::new(config2);
    exporter2.export(make_chunk("Doc 3")).unwrap();

    let content = fs::read_to_string(tmp.path().join("persist.jsonl")).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 4, "should have 4 lines after reopen+write");
}

// ── Error paths ───────────────────────────────────────────────────────────

#[test]
fn config_accessor_returns_config() {
    let tmp = TempDir::new().unwrap();
    let config = ExporterConfig::new(tmp.path().to_path_buf(), ExportFormat::Jsonl, "test");
    let exporter = JsonlExporter::new(config);
    assert_eq!(exporter.config().filename, "test");
}
