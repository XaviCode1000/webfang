//! Concurrency and race condition tests
//!
//! Verifies that concurrent operations on shared resources (files, state)
//! are safe and produce correct results.
//!
//! Run with: cargo nextest run --test-threads 2 concurrency_tests

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tempfile::TempDir;
use tokio::task;
use uuid::Uuid;

use rust_scraper::domain::entities::{DocumentChunk, ExportFormat, ExportState};
use rust_scraper::domain::exporter::{Exporter, ExporterConfig};
use rust_scraper::infrastructure::export::jsonl_exporter::JsonlExporter;
use rust_scraper::infrastructure::export::vector_exporter::VectorExporter;

/// Create a test DocumentChunk with unique title
fn make_chunk(title: &str) -> rust_scraper::domain::DocumentChunkValidated {
    use rust_scraper::domain::{ScrapedContent, ValidUrl};

    let scraped = ScrapedContent {
        title: title.to_string(),
        content: format!("Content for {}", title),
        url: ValidUrl::parse(&format!("https://example.com/page-{}", title.replace(' ', "-"))).expect("valid URL"),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
    };

    let unvalidated: rust_scraper::domain::DocumentChunkUnvalidated = scraped.into();
    unvalidated.validate().expect("valid document")
}

/// Create an ExporterConfig for JSONL with the given temp dir
fn jsonl_config(dir: PathBuf, filename: &str, append: bool) -> ExporterConfig {
    ExporterConfig::new(dir, ExportFormat::Jsonl, filename).with_append(append)
}

/// Create an ExporterConfig for Vector with the given temp dir
fn vector_config(dir: PathBuf, filename: &str, append: bool) -> ExporterConfig {
    ExporterConfig::new(dir, ExportFormat::Vector, filename).with_append(append)
}

// ============================================================================
// Test 1: Concurrent JSONL writes — verify no data loss
// ============================================================================

#[tokio::test]
async fn concurrent_jsonl_exports_no_data_loss() {
    // Arrange
    let temp_dir = TempDir::new().expect("create temp dir");
    let config = jsonl_config(temp_dir.path().to_path_buf(), "concurrent", false);
    let exporter = Arc::new(JsonlExporter::new(config));

    let num_tasks = 10;
    let docs_per_task = 5;

    // Act — spawn concurrent tasks, each exporting documents
    let mut handles = Vec::new();
    for task_id in 0..num_tasks {
        let exporter = Arc::clone(&exporter);
        let handle = task::spawn(async move {
            let mut results = Vec::new();
            for doc_id in 0..docs_per_task {
                let title = format!("task{}-doc{}", task_id, doc_id);
                let chunk = make_chunk(&title);
                let result = exporter.export(chunk);
                results.push(result.is_ok());
            }
            results
        });
        handles.push(handle);
    }

    // Wait for all tasks
    let all_results: Vec<Vec<bool>> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|h| h.expect("task should not panic"))
        .collect();

    // Assert — all exports succeeded
    let total_ok: usize = all_results
        .iter()
        .map(|r| r.iter().filter(|&&x| x).count())
        .sum();
    assert_eq!(
        total_ok,
        num_tasks * docs_per_task,
        "all concurrent exports should succeed"
    );

    // Verify file integrity — all lines should be valid JSON
    let output_path = temp_dir.path().join("concurrent.jsonl");
    assert!(output_path.exists(), "output file should exist");

    let content = std::fs::read_to_string(&output_path).expect("read output file");
    let lines: Vec<&str> = content.lines().collect();

    // All lines must be valid JSON
    for (i, line) in lines.iter().enumerate() {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "line {} should be valid JSON: {}",
            i,
            line
        );
    }

    // Should have exactly the number of documents we wrote
    assert_eq!(
        lines.len(),
        num_tasks * docs_per_task,
        "should have exactly {} lines (no data loss or duplication)",
        num_tasks * docs_per_task
    );
}

// ============================================================================
// Test 2: StateStore concurrent mark_processed — no duplicates
// ============================================================================

#[test]
fn state_store_concurrent_mark_processed_no_duplicates() {
    // Arrange

    let num_threads = 8;
    let urls_per_thread = 100;

    // Use Arc<Mutex<>> for shared mutable state in sync context
    let state = std::sync::Arc::new(std::sync::Mutex::new(ExportState::new(
        "concurrent-test.com",
    )));

    // Act — spawn threads that concurrently mark URLs as processed
    let mut handles = Vec::new();
    for thread_id in 0..num_threads {
        let state = Arc::clone(&state);
        let handle = std::thread::spawn(move || {
            for url_id in 0..urls_per_thread {
                let url = format!("https://concurrent-test.com/page-{}-{}", thread_id, url_id);
                let mut guard = state.lock().expect("lock not poisoned");
                guard.mark_processed(&url);
            }
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("thread should not panic");
    }

    // Assert — all unique URLs should be tracked
    let final_state = state.lock().expect("lock not poisoned");
    let expected_count = num_threads * urls_per_thread;
    assert_eq!(
        final_state.processed_urls.len(),
        expected_count,
        "should track all {} unique URLs without duplicates",
        expected_count
    );

    // Verify no duplicates (ExportState uses Vec with contains check)
    let mut sorted_urls = final_state.processed_urls.clone();
    sorted_urls.sort();
    sorted_urls.dedup();
    assert_eq!(
        sorted_urls.len(),
        expected_count,
        "no duplicate URLs should exist"
    );
}

// ============================================================================
// Test 3: VectorExporter concurrent batch writes — no corruption
// ============================================================================

#[tokio::test]
async fn concurrent_vector_exports_no_corruption() {
    // Arrange
    let temp_dir = TempDir::new().expect("create temp dir");
    let config = vector_config(temp_dir.path().to_path_buf(), "concurrent_vec", true);
    let exporter = Arc::new(VectorExporter::new(config));

    let num_tasks = 6;
    let docs_per_task = 3;

    // Act — spawn concurrent batch exports
    let mut handles = Vec::new();
    for task_id in 0..num_tasks {
        let exporter = Arc::clone(&exporter);
        let handle = task::spawn(async move {
            let docs: Vec<rust_scraper::domain::DocumentChunkValidated> = (0..docs_per_task)
                .map(|doc_id| make_chunk(&format!("vec-task{}-doc{}", task_id, doc_id)))
                .collect();
            exporter.export_batch(&docs)
        });
        handles.push(handle);
    }

    let results: Vec<_> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|h| h.expect("task should not panic"))
        .collect();

    // Assert — all batch exports should succeed
    let success_count = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(
        success_count, num_tasks,
        "all concurrent batch exports should succeed"
    );

    // Verify output file is valid JSON
    let output_path = temp_dir.path().join("concurrent_vec.json");
    assert!(output_path.exists(), "output file should exist");

    let content = std::fs::read_to_string(&output_path).expect("read output file");
    let json: serde_json::Value = serde_json::from_str(&content)
        .expect("output should be valid JSON even after concurrent writes");

    // Verify document count in the array
    let docs = json["documents"]
        .as_array()
        .expect("documents should be an array");
    assert_eq!(
        docs.len(),
        num_tasks * docs_per_task,
        "should have all documents after concurrent writes"
    );
}

// ============================================================================
// Test 4: Concurrent JSONL append mode — preserve existing content
// ============================================================================

#[tokio::test]
async fn concurrent_jsonl_append_preserves_content() {
    // Arrange
    let temp_dir = TempDir::new().expect("create temp dir");

    // Write initial batch without append
    let config_initial = jsonl_config(temp_dir.path().to_path_buf(), "append_test", false);
    let initial_exporter = JsonlExporter::new(config_initial);
    let initial_docs: Vec<rust_scraper::domain::DocumentChunkValidated> = (0..5)
        .map(|i| make_chunk(&format!("initial-{}", i)))
        .collect();
    initial_exporter
        .export_batch(&initial_docs)
        .expect("initial batch should succeed");

    let initial_count = std::fs::read_to_string(temp_dir.path().join("append_test.jsonl"))
        .expect("read initial file")
        .lines()
        .count();
    assert_eq!(initial_count, 5, "initial file should have 5 lines");

    // Act — concurrent appends
    let config_append = jsonl_config(temp_dir.path().to_path_buf(), "append_test", true);
    let exporter = Arc::new(JsonlExporter::new(config_append));

    let num_appenders = 4;
    let docs_per_appender = 3;

    let mut handles = Vec::new();
    for appender_id in 0..num_appenders {
        let exporter = Arc::clone(&exporter);
        let handle = task::spawn(async move {
            for doc_id in 0..docs_per_appender {
                let chunk = make_chunk(&format!("appender{}-doc{}", appender_id, doc_id));
                let _ = exporter.export(chunk);
            }
        });
        handles.push(handle);
    }

    futures::future::join_all(handles).await;

    // Assert — initial content preserved + new content appended
    let final_content = std::fs::read_to_string(temp_dir.path().join("append_test.jsonl"))
        .expect("read final file");
    let final_lines: Vec<&str> = final_content.lines().collect();

    let expected_total = initial_count + (num_appenders * docs_per_appender);
    assert_eq!(
        final_lines.len(),
        expected_total,
        "should have initial ({}) + appended ({}) = {} lines",
        initial_count,
        num_appenders * docs_per_appender,
        expected_total
    );

    // All lines should still be valid JSON
    for line in &final_lines {
        assert!(
            serde_json::from_str::<serde_json::Value>(line).is_ok(),
            "each line should be valid JSON"
        );
    }
}
