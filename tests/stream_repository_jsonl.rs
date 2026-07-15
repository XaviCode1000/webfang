//! T3-D integration test for the headless `StreamRepository` JSONL sink (D3).
//!
//! Drives `StreamRepository` directly and asserts the emitted JSONL:
//! - each line carries an explicit **384-dim** `embedding` array, and
//! - `sha256_hex` is non-empty.
//!
//! Also exercises **Q3**: a chunk with `embedding = None` (the no-`ai` path) is
//! omitted — no null/zero vector line is written.
//!
//! `StreamRepository` has no SQLite dependency, so this test runs under the
//! default (core) build.

use std::sync::Arc;

use webfang::domain::repository::VectorRepository;
use webfang::infrastructure::stream::{StreamRepository, VectorRecord};
use tempfile::NamedTempFile;

#[tokio::test]
async fn stream_repository_writes_jsonl_with_384_dim_embedding() {
    let tmp = NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = Arc::new(StreamRepository::new(&path).expect("open stream"));

    let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();

    repo.save_resource("https://example.com/p", "Page Title", "hash123", 10)
        .await
        .expect("save_resource");

    // Embedded chunk → must be written as one JSONL line.
    repo.save_chunk(
        "hash123-0",
        "https://example.com/p",
        0,
        "cleaned chunk text",
        Some(&embedding),
    )
    .await
    .expect("save_chunk (with embedding)");

    // Q3: no embedding (no-`ai` path) → record omitted.
    repo.save_chunk(
        "hash123-1",
        "https://example.com/p",
        1,
        "chunk without embedding",
        None,
    )
    .await
    .expect("save_chunk (without embedding)");

    let contents = std::fs::read_to_string(&path).expect("read stream");
    let lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1, "only the embedded chunk is written (Q3)");

    let record: VectorRecord = serde_json::from_str(lines[0]).expect("valid JSONL VectorRecord");
    assert!(
        !record.sha256_hex.is_empty(),
        "sha256_hex must be non-empty"
    );
    assert_eq!(
        record.embedding.len(),
        384,
        "explicit 384-dim embedding array expected"
    );
    assert_eq!(record.title.as_deref(), Some("Page Title"));
    assert_eq!(record.chunk_text, "cleaned chunk text");
}

#[tokio::test]
async fn stream_repository_stdout_dash_is_valid_sink_path() {
    // `-` selects stdout; just assert construction succeeds (no file created).
    let repo = StreamRepository::new("-");
    assert!(
        repo.is_ok(),
        "`StreamRepository::new(\"-\")` must construct without error"
    );
}
