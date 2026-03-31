//! JSONL Exporter implementation
//!
//! Exports DocumentChunk to JSON Lines format (one JSON object per line).
//! Optimized for streaming writes and large datasets.

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use fs2::FileExt;

use crate::domain::entities::DocumentChunk;
use crate::domain::exporter::{ExportResult, ExporterConfig, ExporterError};

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
    fn get_writer(&self) -> ExportResult<BufWriter<std::fs::File>> {
        let path = self.config.output_path();

        // Create parent directory if it doesn't exist
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(ExporterError::DirectoryCreation)?;
        }

        // Acquire exclusive file lock to prevent concurrent writes
        let lock_path = path.with_extension("jsonl.lock");
        let lock_file = fs::File::create(&lock_path)
            .map_err(|e| ExporterError::WriteError(format!("{}: {}", lock_path.display(), e)))?;
        // allow: fs2::FileExt::lock_exclusive, clippy misidentifies as std::io::FileExt (1.89+)
        #[allow(clippy::incompatible_msrv)]
        lock_file.lock_exclusive().map_err(|e| {
            ExporterError::WriteError(format!("failed to acquire file lock: {}", e))
        })?;

        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(self.config.append)
            .truncate(!self.config.append)
            .open(&path)
            .map_err(|e| ExporterError::WriteError(format!("{}: {}", path.display(), e)))?;

        // Lock released automatically on drop (RAII)
        Ok(BufWriter::new(file))
    }

    /// Serialize a single document to JSON line
    fn serialize_line(&self, doc: &DocumentChunk) -> ExportResult<String> {
        serde_json::to_string(doc).map_err(ExporterError::Serialization)
    }
}

impl crate::domain::exporter::Exporter for JsonlExporter {
    fn export(&self, document: DocumentChunk) -> ExportResult<()> {
        let line = self.serialize_line(&document)?;
        let mut writer = self.get_writer()?;
        writer.write_all(line.as_bytes())?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        tracing::debug!("Exported document to JSONL: {}", document.id);
        Ok(())
    }

    fn export_batch(&self, documents: Vec<DocumentChunk>) -> ExportResult<()> {
        let count = documents.len();
        let mut writer = self.get_writer()?;

        for doc in documents {
            let line = self.serialize_line(&doc)?;
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

    use crate::domain::entities::ExportFormat;
    use crate::domain::exporter::Exporter;

    use super::*;

    fn create_test_chunk(title: &str) -> DocumentChunk {
        use chrono::Utc;
        use uuid::Uuid;

        DocumentChunk {
            id: Uuid::new_v4(),
            url: "https://example.com/test".to_string(),
            title: title.to_string(),
            content: "Test content".to_string(),
            metadata: std::collections::HashMap::new(),
            timestamp: Utc::now(),
            embeddings: None,
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

        let result = exporter.export_batch(chunks);
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
}
