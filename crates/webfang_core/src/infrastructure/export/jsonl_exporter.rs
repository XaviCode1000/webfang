//! JSONL Exporter implementation
//!
//! Exports DocumentChunk to JSON Lines format (one JSON object per line).
//! Optimized for streaming writes and large datasets.
//!
//! ## Metadata Schema (v2.1.0)
//!
//! Each JSONL line includes:
//! - `url`: Source URL
//! - `timestamp_utc`: ISO 8601 / RFC 3339 timestamp
//! - `title`: Document title (optional)
//! - `content`: Extracted text content
//! - `checksum_sha256`: SHA-256 hash of content for deduplication
//! - `metadata_version`: Schema version ("2.1.0")
//! - `content_length`: Character count for quick filtering
//! - `word_count`: Word count (optional, from metadata or computed)
//! - `reading_time`: Estimated reading time in minutes (optional)
//! - `language`: Detected language (optional)
//! - `content_type`: Content type classification (optional)
//! - `scrape_date`: Date of scrape (optional)
//! - `extra_metadata`: Additional metadata HashMap (optional)

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use serde::Serialize;

use crate::domain::entities::DocumentChunkValidated;
use crate::domain::exporter::{ExportResult, ExporterConfig, ExporterError};

/// RAII wrapper around an exclusive file lock. While alive it holds the lock;
/// on drop it releases the lock **and deletes the lock file** so no `.lock`
/// orphan is left behind (issue #582).
///
/// `#[must_use]` warns if a caller acquires the lock but lets it drop
/// immediately (a likely bug — the lock would be released before any write).
#[must_use]
struct FileLock {
    handle: std::fs::File,
    lock_path: PathBuf,
}

impl FileLock {
    /// Acquire an exclusive lock at `<path>.jsonl.lock`.
    fn acquire(path: &Path) -> ExportResult<Self> {
        let lock_path = path.with_extension("jsonl.lock");
        // M1 FIX: Write PID metadata to lock file for debugging
        let mut lock_file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)
            .map_err(|e| ExporterError::WriteError(format!("{}: {}", lock_path.display(), e)))?;
        use std::io::Write;
        let _ = writeln!(lock_file, "pid={} op=exclusive_write", std::process::id());
        // allow: fs2::FileExt::lock_exclusive, clippy misidentifies as std::io::FileExt (1.89+)
        #[allow(clippy::incompatible_msrv)]
        lock_file
            .lock_exclusive()
            .map_err(|e| ExporterError::WriteError(format!("failed to acquire file lock: {e}")))?;
        Ok(Self {
            handle: lock_file,
            lock_path,
        })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        // Release the OS-level exclusive lock, then delete the lock file. Both
        // best-effort: a failure here must not mask the real export result.
        // Fully qualified syntax: avoids unstable_name_collisions with future
        // std::fs::File::unlock (rust-lang/rust#48919).
        let _ = fs2::FileExt::unlock(&self.handle);
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// Webfang JSONL metadata schema (v2.1.0)
///
/// Wraps DocumentChunkValidated with additional fields for
/// RAG pipeline integration and content deduplication.
#[derive(Serialize)]
pub struct WebfangMetadata<'a> {
    /// Source URL
    pub url: &'a str,
    /// ISO 8601 / RFC 3339 timestamp
    pub timestamp_utc: String,
    /// Document title (optional)
    pub title: Option<&'a str>,
    /// Extracted text content
    pub content: &'a str,
    /// SHA-256 hash of content for deduplication
    pub checksum_sha256: String,
    /// Schema version
    pub metadata_version: &'static str,
    /// Character count for quick filtering
    pub content_length: usize,
    /// Word count (from metadata or computed from content)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word_count: Option<usize>,
    /// Estimated reading time in minutes (from metadata or computed)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading_time: Option<usize>,
    /// Detected language
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Content type classification
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Date of scrape (ISO 8601)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrape_date: Option<String>,
    /// Additional metadata (excerpt, author, etc.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_metadata: Option<std::collections::HashMap<String, String>>,
}

impl<'a> WebfangMetadata<'a> {
    /// Create from a DocumentChunkValidated
    pub fn from_chunk(chunk: &'a DocumentChunkValidated) -> Self {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(chunk.content.as_bytes());
        let checksum = format!("{:x}", hasher.finalize());

        let word_count = chunk
            .metadata
            .get("word_count")
            .and_then(|v| v.parse::<usize>().ok())
            .or_else(|| {
                let count = chunk.content.split_whitespace().count();
                if count > 0 { Some(count) } else { None }
            });

        let reading_time = chunk
            .metadata
            .get("reading_time")
            .and_then(|v| v.parse::<usize>().ok())
            .or_else(|| word_count.map(|wc| (wc / 200).max(1)));

        let language = chunk.metadata.get("language").cloned();
        let content_type = chunk.metadata.get("content_type").cloned();
        let scrape_date = chunk.metadata.get("scrape_date").cloned();

        let extra_metadata = if chunk.metadata.is_empty() {
            None
        } else {
            Some(chunk.metadata.clone())
        };

        Self {
            url: &chunk.url,
            timestamp_utc: chunk.timestamp.to_rfc3339(),
            title: Some(&chunk.title),
            content: &chunk.content,
            checksum_sha256: checksum,
            metadata_version: "2.1.0",
            content_length: chunk.content.chars().count(),
            word_count,
            reading_time,
            language,
            content_type,
            scrape_date,
            extra_metadata,
        }
    }
}

/// JSONL Exporter - writes one JSON object per line
///
/// Optimized for:
/// - Streaming writes (no in-memory buffering of entire dataset)
/// - Large datasets (appends to existing files)
/// - Integration with RAG pipelines (jq, pandas compatible)
#[derive(Debug)]
pub struct JsonlExporter {
    config: ExporterConfig,
}

impl JsonlExporter {
    /// Create a new JsonlExporter with the given configuration
    #[must_use]
    pub fn new(config: ExporterConfig) -> Self {
        Self { config }
    }

    /// Create from output directory and filename
    #[must_use]
    pub fn new_with_path(output_dir: PathBuf, filename: impl Into<String>) -> Self {
        let config = ExporterConfig::new(output_dir, crate::domain::ExportFormat::Jsonl, filename)
            .with_append(true);
        Self::new(config)
    }

    /// Get the file handle, creating directory if needed
    fn writer(&self) -> ExportResult<BufWriter<std::fs::File>> {
        let path = self.config.output_path();

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(ExporterError::DirectoryCreation)?;
        }

        // Acquire an exclusive file lock for the duration of this write, then
        // release it (and delete the lock file) when the returned writer is
        // dropped. A short-lived lock avoids blocking a second exporter pointing
        // at the same file (e.g. the append path in `test_jsonl_exporter_append`),
        // and deleting the file prevents orphaned `.lock` leftovers (issue #582).
        let _lock = FileLock::acquire(&path)?;

        // Fix: Only truncate if file doesn't exist yet.
        // Prevents data loss when writer() is called multiple times
        // (once per document in export(), once per batch in export_batch())
        let file_exists = path.exists();
        // When file exists: open in append mode (preserve content).
        // When file does not exist: create + write + truncate (fresh start).
        // Split into two branches to avoid clippy warning on .write + .append.
        let file = if file_exists {
            OpenOptions::new().create(true).append(true).open(&path)
        } else {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
        }
        .map_err(|e| ExporterError::WriteError(format!("{}: {}", path.display(), e)))?;

        Ok(BufWriter::new(file))
    }

    /// Serialize a single document to JSON line with WebfangMetadata
    fn serialize_line(&self, doc: &DocumentChunkValidated) -> ExportResult<String> {
        let metadata = WebfangMetadata::from_chunk(doc);
        serde_json::to_string(&metadata).map_err(ExporterError::Serialization)
    }
}

impl crate::domain::exporter::Exporter for JsonlExporter {
    fn export(&self, document: DocumentChunkValidated) -> ExportResult<()> {
        let line = self.serialize_line(&document)?;
        let mut writer = self.writer()?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        tracing::debug!("Exported document to JSONL: {}", document.id);
        Ok(())
    }

    #[tracing::instrument(skip(self, documents), fields(exporter = "jsonl", documents = documents.len()))]
    fn export_batch(&self, documents: &[DocumentChunkValidated]) -> ExportResult<()> {
        let count = documents.len();
        let mut writer = self.writer()?;

        for doc in documents {
            let line = self.serialize_line(doc)?;
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
        }

        writer.flush()?;
        tracing::info!("Batch exported {} documents to JSONL", count);
        Ok(())
    }

    fn config(&self) -> &ExporterConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use tempfile::TempDir;

    use crate::domain::config::ExportFormat;
    use crate::domain::exporter::Exporter;

    use super::*;

    fn create_test_chunk(title: &str) -> DocumentChunkValidated {
        use crate::domain::Validated;
        use chrono::Utc;
        use uuid::Uuid;

        crate::domain::DocumentChunkValidated {
            id: Uuid::new_v4(),
            url: "https://example.com/test".to_string(),
            title: title.to_string(),
            content: "Test content".to_string(),
            metadata: std::collections::HashMap::new(),
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: None,
            _state: std::marker::PhantomData::<Validated>,
        }
    }

    #[test]
    fn test_jsonl_exporter_single_document() {
        let temp_dir = TempDir::new().unwrap();
        let config =
            ExporterConfig::new(PathBuf::from(temp_dir.path()), ExportFormat::Jsonl, "test")
                .with_append(false);

        let exporter = JsonlExporter::new(config);
        let chunk = create_test_chunk("Test Title");

        let result = exporter.export(chunk);
        assert!(result.is_ok());

        // Verify file exists and has valid JSONL
        let output_path = temp_dir.path().join("test.jsonl");
        assert!(output_path.exists());

        let content = fs::read_to_string(&output_path).unwrap();
        assert!(!content.is_empty());
        // Each line should be valid JSON
        for line in content.lines() {
            assert!(serde_json::from_str::<serde_json::Value>(line).is_ok());
        }
    }

    #[test]
    fn test_jsonl_exporter_batch() {
        let temp_dir = TempDir::new().unwrap();
        let config = ExporterConfig::new(
            PathBuf::from(temp_dir.path()),
            ExportFormat::Jsonl,
            "batch_test",
        )
        .with_append(false);

        let exporter = JsonlExporter::new(config);
        let chunks = vec![
            create_test_chunk("Title 1"),
            create_test_chunk("Title 2"),
            create_test_chunk("Title 3"),
        ];

        let result = exporter.export_batch(&chunks);
        assert!(result.is_ok());

        let output_path = temp_dir.path().join("batch_test.jsonl");
        let content = fs::read_to_string(&output_path).unwrap();

        // Should have 3 lines
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3);
    }

    #[test]
    fn test_jsonl_exporter_append() {
        let temp_dir = TempDir::new().unwrap();

        // First write
        let config1 = ExporterConfig::new(
            PathBuf::from(temp_dir.path()),
            ExportFormat::Jsonl,
            "append_test",
        )
        .with_append(false);

        let exporter1 = JsonlExporter::new(config1);
        exporter1.export(create_test_chunk("First")).unwrap();

        // Second write with append
        let config2 = ExporterConfig::new(
            PathBuf::from(temp_dir.path()),
            ExportFormat::Jsonl,
            "append_test",
        )
        .with_append(true);

        let exporter2 = JsonlExporter::new(config2);
        exporter2.export(create_test_chunk("Second")).unwrap();

        // Should have 2 lines
        let output_path = temp_dir.path().join("append_test.jsonl");
        let content = fs::read_to_string(&output_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_webfang_metadata_v210_schema_version() {
        let chunk = create_test_chunk("Version Test");
        let metadata = WebfangMetadata::from_chunk(&chunk);
        assert_eq!(metadata.metadata_version, "2.1.0");
    }

    #[test]
    fn test_webfang_metadata_v210_serialization_empty_metadata() {
        let chunk = create_test_chunk("Empty Meta");
        let metadata = WebfangMetadata::from_chunk(&chunk);
        let json = serde_json::to_string(&metadata).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["metadata_version"], "2.1.0");
        // word_count and reading_time are computed from content when not in metadata
        assert!(value.get("word_count").is_some());
        assert!(value.get("reading_time").is_some());
        // These are only present when metadata provides them
        assert!(value.get("language").is_none());
        assert!(value.get("content_type").is_none());
        assert!(value.get("scrape_date").is_none());
        assert!(value.get("extra_metadata").is_none());
    }

    #[test]
    fn test_webfang_metadata_v210_with_rich_metadata() {
        use crate::domain::Validated;
        use chrono::Utc;
        use uuid::Uuid;

        let mut meta = std::collections::HashMap::new();
        meta.insert("word_count".to_string(), "42".to_string());
        meta.insert("reading_time".to_string(), "2".to_string());
        meta.insert("language".to_string(), "es".to_string());
        meta.insert("content_type".to_string(), "article".to_string());
        meta.insert("scrape_date".to_string(), "2026-08-10".to_string());
        meta.insert("author".to_string(), "Jane Doe".to_string());
        meta.insert("excerpt".to_string(), "A summary".to_string());

        let chunk = crate::domain::DocumentChunkValidated {
            id: Uuid::new_v4(),
            url: "https://example.com/rich".to_string(),
            title: "Rich Metadata".to_string(),
            content: "This is the main content body.".to_string(),
            metadata: meta,
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: None,
            _state: std::marker::PhantomData::<Validated>,
        };

        let metadata = WebfangMetadata::from_chunk(&chunk);
        let json = serde_json::to_string(&metadata).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["metadata_version"], "2.1.0");
        assert_eq!(value["word_count"], 42);
        assert_eq!(value["reading_time"], 2);
        assert_eq!(value["language"], "es");
        assert_eq!(value["content_type"], "article");
        assert_eq!(value["scrape_date"], "2026-08-10");

        let extra = value["extra_metadata"].as_object().unwrap();
        assert_eq!(extra["author"], "Jane Doe");
        assert_eq!(extra["excerpt"], "A summary");
    }

    #[test]
    fn test_webfang_metadata_v210_computed_word_count() {
        use crate::domain::Validated;
        use chrono::Utc;
        use uuid::Uuid;

        let chunk = crate::domain::DocumentChunkValidated {
            id: Uuid::new_v4(),
            url: "https://example.com/auto".to_string(),
            title: "Auto Word Count".to_string(),
            content: "one two three four five".to_string(),
            metadata: std::collections::HashMap::new(),
            timestamp: Utc::now(),
            embeddings: None,
            correlation_id: None,
            _state: std::marker::PhantomData::<Validated>,
        };

        let metadata = WebfangMetadata::from_chunk(&chunk);
        assert_eq!(metadata.word_count, Some(5));
        assert_eq!(metadata.reading_time, Some(1));
    }
}
