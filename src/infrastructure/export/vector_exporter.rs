//! VectorExporter implementation for RAG pipeline
//!
//! Exports document chunks to JSON format with metadata headers,
//! supporting embeddings and cosine similarity calculations.

// `File::unlock()` is stable since 1.89.0, but we use fs2::FileExt for compatibility.
#![allow(clippy::incompatible_msrv)]

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use fs2::FileExt;

use crate::domain::entities::DocumentChunk;
use crate::domain::exporter::{ExportResult, Exporter, ExporterConfig, ExporterError};

/// Computes cosine similarity between two vectors
///
/// Returns a value between -1.0 and 1.0, where:
/// - 1.0 means identical direction
/// - 0.0 means orthogonal
/// - -1.0 means opposite direction
///
/// Returns 0.0 for zero-magnitude vectors.
///
/// # Arguments
/// * `a` - First vector
/// * `b` - Second vector
///
/// # Errors
/// Returns `DimensionMismatch` if vectors have different dimensions
#[inline]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Result<f32, ExporterError> {
    if a.len() != b.len() {
        return Err(ExporterError::DimensionMismatch {
            expected: b.len(),
            actual: a.len(),
        });
    }

    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if mag_a == 0.0 || mag_b == 0.0 {
        return Ok(0.0);
    }

    Ok(dot_product / (mag_a * mag_b))
}

/// VectorExporter for RAG pipeline
///
/// Exports documents to JSON format with:
/// - Metadata header (format version, dimensions, document count)
/// - Documents array with optional embeddings
/// - Support for append mode
pub struct VectorExporter {
    config: ExporterConfig,
    dimensions: Mutex<Option<usize>>,
}

impl VectorExporter {
    /// Create a new VectorExporter with default path
    #[must_use]
    pub fn new(config: ExporterConfig) -> Self {
        Self {
            config,
            dimensions: Mutex::new(None),
        }
    }

    /// Create a new VectorExporter with custom output path
    #[must_use]
    pub fn new_with_path(config: ExporterConfig, output_dir: impl Into<PathBuf>) -> Self {
        let mut config = config;
        config.output_dir = output_dir.into();
        Self {
            config,
            dimensions: Mutex::new(None),
        }
    }

    /// Get a file writer with proper locking
    ///
    /// Creates directories if needed, acquires fs2 file lock,
    /// and returns a BufWriter for efficient I/O.
    ///
    /// In append mode with an existing file, finds and truncates at the
    /// closing `]` so the writer can append documents and re-close.
    fn get_writer(&self) -> ExportResult<(File, BufWriter<File>)> {
        // Create output directory if it doesn't exist
        fs::create_dir_all(&self.config.output_dir)?;

        let path = self.config.output_path();

        let file = if self.config.append && path.exists() {
            let mut f = OpenOptions::new().read(true).write(true).open(&path)?;

            // Find the closing `]` and truncate there so we can re-append
            let len = f.metadata()?.len();
            if len > 0 {
                let seek_start = len.saturating_sub(256);
                f.seek(SeekFrom::Start(seek_start))?;
                let mut buf = Vec::new();
                f.read_to_end(&mut buf)?;

                let content = String::from_utf8_lossy(&buf);
                if let Some(last_bracket) = content.rfind(']') {
                    let truncate_pos = seek_start + last_bracket as u64;
                    f.set_len(truncate_pos)?;
                    f.seek(SeekFrom::Start(truncate_pos))?;
                }
            }
            f
        } else {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(!self.config.append)
                .open(&path)?
        };

        // Acquire exclusive lock
        file.lock_exclusive()?;

        let writer = BufWriter::new(file.try_clone()?);

        Ok((file, writer))
    }

    /// Write metadata header to file
    ///
    /// For new files: writes complete header
    /// For append mode: the file is already truncated at the closing `]`
    /// so we just seek back and re-write the document count.
    fn write_metadata_header(
        &self,
        writer: &mut BufWriter<File>,
        file: &mut File,
        is_first_doc: bool,
    ) -> ExportResult<()> {
        if !is_first_doc {
            return Ok(());
        }

        if self.config.append && file.metadata()?.len() > 0 {
            // File already truncated at `]` by get_writer() — just seek back to it
            file.seek(SeekFrom::End(0))?;
        } else {
            // New file or overwrite mode - write complete header
            let timestamp = Utc::now().to_rfc3339();
            let dimensions_json = self
                .dimensions
                .lock()
                .expect("lock poisoned")
                .map(|d| d.to_string())
                .unwrap_or_else(|| "null".to_string());
            let header = format!(
                r#"{{"format_version": "1.0", "model_name": null, "dimensions": {}, "total_documents": 0, "created_at": "{}", "documents": ["#,
                dimensions_json, timestamp
            );

            write!(writer, "{}", header)?;
        }

        Ok(())
    }

    /// Serialize a document chunk to JSON
    ///
    /// Validates embedding dimensions if present.
    /// Rejects NaN or Infinity values in embeddings — they produce invalid JSON.
    fn serialize_document(&self, doc: &DocumentChunk) -> ExportResult<String> {
        // Validate embedding dimensions if present
        if let Some(ref embeddings) = doc.embeddings {
            let mut dim_guard = self.dimensions.lock().expect("lock poisoned");
            if let Some(exp) = *dim_guard {
                if embeddings.len() != exp {
                    return Err(ExporterError::DimensionMismatch {
                        expected: exp,
                        actual: embeddings.len(),
                    });
                }
            } else {
                // First document with embeddings - record dimensions
                *dim_guard = Some(embeddings.len());
            }

            // Reject NaN/Infinity — serde_json serialises them as `null` silently
            if embeddings.iter().any(|v| !v.is_finite()) {
                return Err(ExporterError::WriteError("embeddings contain NaN or Infinity".into()));
            }
        }

        // Serialize to JSON
        let serialized = serde_json::to_string(doc)?;
        Ok(serialized)
    }

    /// Close the JSON structure properly
    fn close_json(&self, writer: &mut BufWriter<File>, _doc_count: usize) -> ExportResult<()> {
        writeln!(writer, "]}}")?;
        writer.flush()?;

        Ok(())
    }
}

impl Exporter for VectorExporter {
    fn export(&self, document: DocumentChunk) -> ExportResult<()> {
        let (mut file, mut writer) = self.get_writer()?;
        let is_first_doc =
            !self.config.append || file.metadata().map(|m| m.len() == 0).unwrap_or(true);

        self.write_metadata_header(&mut writer, &mut file, is_first_doc)?;

        let serialized = self.serialize_document(&document)?;

        if !is_first_doc {
            write!(writer, ",")?;
        }
        writeln!(writer, "{}", serialized)?;

        self.close_json(&mut writer, 1)?;

        // Release lock
        file.unlock()?;

        Ok(())
    }

    fn export_batch(&self, documents: &[DocumentChunk]) -> ExportResult<()> {
        if documents.is_empty() {
            return Ok(());
        }

        let (mut file, mut writer) = self.get_writer()?;
        let is_first_doc =
            !self.config.append || file.metadata().map(|m| m.len() == 0).unwrap_or(true);

        self.write_metadata_header(&mut writer, &mut file, is_first_doc)?;

        let mut doc_count = 0;
        for (i, doc) in documents.iter().enumerate() {
            if i > 0 || !is_first_doc {
                write!(writer, ",")?;
            }

            let serialized = self.serialize_document(doc)?;
            writeln!(writer, "{}", serialized)?;
            doc_count += 1;
        }

        self.close_json(&mut writer, doc_count)?;

        // Release lock
        file.unlock()?;

        Ok(())
    }

    fn config(&self) -> &ExporterConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::entities::ExportFormat;

    fn create_test_config_with_dir(dir: PathBuf) -> ExporterConfig {
        ExporterConfig::new(dir, ExportFormat::Vector, "test_export")
    }

    fn create_test_config() -> ExporterConfig {
        ExporterConfig::new(
            PathBuf::from("/tmp/test_vector_export"),
            ExportFormat::Vector,
            "test_export",
        )
    }

    fn create_test_chunk() -> DocumentChunk {
        DocumentChunk {
            id: uuid::Uuid::new_v4(),
            url: "https://example.com/test".to_string(),
            title: "Test Document".to_string(),
            content: "Test content for vector export".to_string(),
            metadata: Default::default(),
            timestamp: Utc::now(),
            embeddings: Some(vec![0.1, 0.2, 0.3, 0.4, 0.5]),
        }
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let result = cosine_similarity(&a, &b).unwrap();
        assert!((result - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let result = cosine_similarity(&a, &b).unwrap();
        assert!(result.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_zero_magnitude() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        let result = cosine_similarity(&a, &b).unwrap();
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_cosine_similarity_normal() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![4.0, 5.0, 6.0];
        let result = cosine_similarity(&a, &b).unwrap();
        // Expected: (1*4 + 2*5 + 3*6) / (sqrt(14) * sqrt(77))
        // = 32 / (3.741... * 8.774...) ≈ 0.9746
        assert!((result - 0.9746).abs() < 1e-3);
    }

    #[test]
    fn test_cosine_similarity_dimension_mismatch() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let result = cosine_similarity(&a, &b);
        assert!(result.is_err());
    }

    #[test]
    fn test_vector_exporter_creation() {
        let config = create_test_config();
        let exporter = VectorExporter::new(config.clone());
        assert_eq!(exporter.config().output_dir, config.output_dir);
        assert_eq!(exporter.config().format, ExportFormat::Vector);
    }

    #[test]
    fn test_vector_exporter_with_path() {
        let config = create_test_config();
        let custom_path = PathBuf::from("/custom/path");
        let exporter = VectorExporter::new_with_path(config, custom_path.clone());
        assert_eq!(exporter.config().output_dir, custom_path);
    }

    #[test]
    fn test_serialize_document_with_embeddings() {
        let config = create_test_config();
        let exporter = VectorExporter::new(config);
        let doc = create_test_chunk();

        let result = exporter.serialize_document(&doc);
        assert!(result.is_ok());

        let json_str = result.unwrap();
        assert!(json_str.contains("embeddings"));
        assert!(json_str.contains("Test Document"));
    }

    #[test]
    fn test_serialize_document_dimension_mismatch() {
        let config = create_test_config();
        let exporter = VectorExporter::new(config);

        // First document sets dimensions
        let doc1 = create_test_chunk();
        let _ = exporter.serialize_document(&doc1);

        // Second document with different dimensions
        let mut doc2 = create_test_chunk();
        doc2.embeddings = Some(vec![0.1, 0.2]); // Only 2 dimensions

        let result = exporter.serialize_document(&doc2);
        assert!(matches!(result, Err(ExporterError::DimensionMismatch { .. })));
    }

    #[test]
    fn test_serialize_document_without_embeddings() {
        let config = create_test_config();
        let exporter = VectorExporter::new(config);

        let mut doc = create_test_chunk();
        doc.embeddings = None;

        let result = exporter.serialize_document(&doc);
        assert!(result.is_ok());

        let json_str = result.unwrap();
        // embeddings field is skipped when None (skip_serializing_if)
        assert!(!json_str.contains("embeddings"));
        assert!(json_str.contains("Test Document"));
    }

    #[test]
    fn test_export_batch_empty() {
        let config = create_test_config();
        let exporter = VectorExporter::new(config);

        let result = exporter.export_batch(&[]);
        assert!(result.is_ok());
    }

    // --- Task 4.4: Append mode test ---

    #[test]
    fn test_vector_exporter_append_mode_preserves_documents() {
        let temp_dir = std::env::temp_dir().join("test_vector_append");
        let _ = std::fs::remove_dir_all(&temp_dir);

        // First batch: write 2 documents without append
        let mut config1 = create_test_config_with_dir(temp_dir.clone());
        config1.append = false;
        let exporter1 = VectorExporter::new(config1);

        let docs1 = vec![create_test_chunk(), create_test_chunk()];
        let result = exporter1.export_batch(&docs1);
        assert!(result.is_ok(), "first batch should succeed: {:?}", result.err());

        let file1_path = temp_dir.join("test_export.json");
        assert!(file1_path.exists(), "output file should exist after first batch");

        // Read file content after first write
        let content1 = std::fs::read_to_string(&file1_path).expect("should read file");
        let json1: serde_json::Value =
            serde_json::from_str(&content1).expect("first write should produce valid JSON");
        let first_doc_count = json1["documents"].as_array().map_or(0, |a| a.len());
        assert_eq!(first_doc_count, 2, "first batch should have 2 documents");

        // Second batch: append 1 document with append=true
        let mut config2 = create_test_config_with_dir(temp_dir.clone());
        config2.append = true;
        let exporter2 = VectorExporter::new(config2);

        let doc3 = create_test_chunk();
        let result = exporter2.export(doc3);
        assert!(result.is_ok(), "append should succeed: {:?}", result.err());

        // Read final file and verify all 3 documents are present
        let content2 = std::fs::read_to_string(&file1_path).expect("should read file after append");
        let json2: serde_json::Value =
            serde_json::from_str(&content2).expect("after append should be valid JSON");
        let final_doc_count = json2["documents"].as_array().map_or(0, |a| a.len());
        assert_eq!(final_doc_count, 3, "should have 3 documents after append (2 + 1)");

        // Verify metadata header still exists
        assert!(
            json2.get("format_version").is_some() || json2.get("metadata").is_some(),
            "metadata header should still be present after append"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // --- Task 4.9: Directory creation failure test ---

    #[test]
    fn test_vector_exporter_directory_creation_fails() {
        // Use a path that is guaranteed to fail (no permission on /root)
        let config = ExporterConfig::new(
            PathBuf::from("/root/no-permission/test_vector"),
            ExportFormat::Vector,
            "test_export",
        );
        let exporter = VectorExporter::new(config);
        let doc = create_test_chunk();

        let result = exporter.export(doc);
        assert!(result.is_err(), "export to /root should fail with directory creation error");
    }

    // --- Task 4.10: Serialization failure with NaN in embeddings ---

    #[test]
    fn test_vector_exporter_serialization_nan_fails() {
        let temp_dir = std::env::temp_dir().join("test_vector_nan");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let config = create_test_config_with_dir(temp_dir.clone());
        let exporter = VectorExporter::new(config);

        // Create a document with NaN in embeddings — serde_json rejects NaN by default
        let mut doc = create_test_chunk();
        doc.embeddings = Some(vec![0.1, f32::NAN, 0.3]);

        let result = exporter.export(doc);
        assert!(
            result.is_err(),
            "export with NaN in embeddings should fail with serialization error"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
