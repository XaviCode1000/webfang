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

use crate::domain::entities::DocumentChunkValidated;
use crate::domain::exporter::{ExportResult, Exporter, ExporterConfig, ExporterError};

/// Reserved character width of the `total_documents` header field.
///
/// The count is written right-aligned (space-padded) into this window so
/// [`VectorExporter::close_json`] can patch it in place without shifting the
/// rest of the file. Covers document counts up to 9,999,999,999.
const COUNT_FIELD_WIDTH: usize = 10;

/// Reserved character width of the `dimensions` header field.
///
/// Right-aligned like [`COUNT_FIELD_WIDTH`]; holds either the embedding
/// dimension count or the literal `null`. Eight characters cover any
/// realistic embedding size.
const DIM_FIELD_WIDTH: usize = 8;

/// Number of leading bytes scanned to locate and patch the metadata header.
///
/// The header line fits comfortably inside this window even with the reserved
/// fields, so scanning a fixed prefix avoids reading whole files on close.
const HEADER_SCAN_BYTES: usize = 512;

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
    fn writer(&self) -> ExportResult<(File, BufWriter<File>)> {
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
                .read(true)
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
    /// For new files: writes the header template with reserved (right-aligned,
    /// space-padded) `dimensions` and `total_documents` windows that
    /// [`Self::close_json`] patches in place once the real values are known.
    /// For append mode with existing documents: the file is already truncated
    /// at the closing `]` by [`Self::writer`], so nothing is written here.
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
            // File already truncated at `]` by writer() — just seek back to it
            file.seek(SeekFrom::End(0))?;
        } else {
            // New file or overwrite mode — header with reserved patch windows
            let timestamp = Utc::now().to_rfc3339();
            let header = self.build_header_line(&timestamp);
            write!(writer, "{header}")?;
        }

        Ok(())
    }

    /// Build the metadata header line with reserved in-place patch windows.
    ///
    /// `dimensions` and `total_documents` are right-aligned into fixed-width
    /// windows ([`DIM_FIELD_WIDTH`] / [`COUNT_FIELD_WIDTH`]). Space padding is
    /// legal JSON whitespace and right-alignment avoids leading zeros, so both
    /// fields can be overwritten later without rewriting the whole file.
    fn build_header_line(&self, created_at: &str) -> String {
        // Mutex poisoning indicates a bug in the calling code, not a recoverable error.
        #[allow(clippy::expect_used)]
        let dimensions_json = self
            .dimensions
            .lock()
            // LCOV_EXCL_LINE defensive: mutex-poisoning — poisoning indicates a bug in the calling code
            .expect("lock poisoned")
            .map(|d| d.to_string())
            .unwrap_or_else(|| "null".to_string());
        let count = 0_usize;
        format!(
            r#"{{"format_version": "1.0", "model_name": null, "dimensions": {dimensions_json:>DIM_FIELD_WIDTH$}, "total_documents": {count:>COUNT_FIELD_WIDTH$}, "created_at": "{created_at}", "documents": ["#
        )
    }

    /// Serialize a document chunk to JSON
    ///
    /// Validates embedding dimensions if present.
    /// Rejects NaN or Infinity values in embeddings — they produce invalid JSON.
    fn serialize_document(&self, doc: &DocumentChunkValidated) -> ExportResult<String> {
        // Validate embedding dimensions if present
        if let Some(ref embeddings) = doc.embeddings {
            // Mutex poisoning indicates a bug in the calling code, not a recoverable error.
            #[allow(clippy::expect_used)]
            // LCOV_EXCL_LINE defensive: mutex-poisoning — poisoning indicates a bug in the calling code
            let mut dim_guard = self.dimensions.lock().expect("lock poisoned");
            if let Some(exp) = *dim_guard {
                if embeddings.len() != exp {
                    // Dimension mismatch: the document is degraded (no embeddings in output).
                    // error! because --output-vectors implies embeddings are the point.
                    tracing::error!(
                        expected_dimensions = exp,
                        actual_dimensions = embeddings.len(),
                        "Dimension mismatch — document serialized WITHOUT embeddings"
                    );
                    // Create a copy without embeddings
                    let mut doc_without_embeddings = doc.clone();
                    doc_without_embeddings.embeddings = None;
                    return serde_json::to_string(&doc_without_embeddings)
                        .map_err(|e| ExporterError::WriteError(e.to_string()));
                }
            } else {
                // First document with embeddings - record dimensions
                *dim_guard = Some(embeddings.len());
            }

            // Reject NaN/Infinity — serde_json serialises them as `null` silently
            if embeddings.iter().any(|v| !v.is_finite()) {
                return Err(ExporterError::WriteError(
                    "embeddings contain NaN or Infinity".into(),
                ));
            }
        }

        // Serialize to JSON
        let serialized = serde_json::to_string(doc)?;
        Ok(serialized)
    }

    /// Read how many documents an existing export already holds.
    ///
    /// Called when appending to a non-empty file. Parses the reserved
    /// `total_documents` window written by [`Self::close_json`]. Files whose
    /// header predates the in-place counter fix (issue #502) fail the
    /// sentinel check and are migrated on the fly by
    /// [`Self::migrate_legacy_header`].
    ///
    /// Leaves the file offset at EOF so the caller can keep appending.
    fn read_append_state(&self, file: &mut File) -> ExportResult<usize> {
        let head = read_header_bytes(file)?;

        let marker = b"\"total_documents\": ";
        let value_start = find_subslice(&head, marker)
            .map(|pos| pos + marker.len())
            .ok_or_else(|| {
                ExporterError::WriteError(
                    "vector export header is missing the total_documents field".to_string(),
                )
            })?;

        // The fixed-width layout guarantees a ',' exactly COUNT_FIELD_WIDTH
        // bytes after the value start. Anything else means the file was
        // written by the pre-fix exporter (or is corrupt) and needs migration.
        let existing = if head.get(value_start + COUNT_FIELD_WIDTH) == Some(&b',') {
            let count = parse_count_field(&head[value_start..value_start + COUNT_FIELD_WIDTH])?;
            if count == 0 {
                // Defensive: files written by this version always carry the
                // true count. Fall back to counting document lines.
                count_documents(file)?
            } else {
                count
            }
        } else {
            self.migrate_legacy_header(file, &head)?
        };

        file.seek(SeekFrom::End(0))?;
        Ok(existing)
    }

    /// Migrate a pre-fix header to the reserved-window layout.
    ///
    /// Legacy files stored a hardcoded `"total_documents": 0` and dimensions
    /// learned too late to be written. The real document count is recovered
    /// from the line structure, the header is rebuilt with the reserved
    /// windows (preserving the original `created_at`), and the file head is
    /// rewritten in place. Safe because the caller holds the exclusive fs2
    /// lock for the whole export call.
    fn migrate_legacy_header(&self, file: &mut File, head: &[u8]) -> ExportResult<usize> {
        // serde_json never emits raw line breaks inside strings, so every
        // 0x0A byte marks the end of exactly one document line.
        let existing = count_documents(file)?;

        let docs_marker = b"\"documents\": [";
        let body_start = find_subslice(head, docs_marker)
            .map(|pos| pos + docs_marker.len())
            .ok_or_else(|| {
                ExporterError::WriteError(
                    "vector export header is missing the documents array".to_string(),
                )
            })?;
        let created_at = extract_created_at(head).ok_or_else(|| {
            ExporterError::WriteError("vector export header is missing created_at".to_string())
        })?;

        file.seek(SeekFrom::Start(body_start as u64))?;
        let mut body = Vec::new();
        file.read_to_end(&mut body)?;

        let new_head = self.build_header_line(&created_at);
        file.seek(SeekFrom::Start(0))?;
        file.write_all(new_head.as_bytes())?;
        file.write_all(&body)?;
        let new_len = file.stream_position()?;
        file.set_len(new_len)?;

        tracing::warn!(
            existing_documents = existing,
            path = %self.config.output_path().display(),
            "Migrating legacy vector export header to the fixed-count layout"
        );

        Ok(existing)
    }

    /// Close the JSON structure and patch the header counters in place.
    ///
    /// Writes the closing bracket, then overwrites the reserved
    /// `total_documents` window (and the `dimensions` window once the
    /// embedding size became known during this run) at their fixed header
    /// offsets. The `file` handle shares its offset with the `BufWriter`'s
    /// clone, so the writer is flushed before any seek.
    fn close_json(
        &self,
        writer: &mut BufWriter<File>,
        file: &mut File,
        total_documents: usize,
    ) -> ExportResult<()> {
        writeln!(writer, "]}}")?;
        writer.flush()?;

        let head = read_header_bytes(file)?;

        let count = format!("{total_documents:>COUNT_FIELD_WIDTH$}");
        patch_header_field(
            file,
            &head,
            b"\"total_documents\": ",
            "total_documents",
            &count,
            COUNT_FIELD_WIDTH,
        )?;

        // Dimensions may have been learned mid-run by serialize_document —
        // backfill the reserved window now that the final value is known.
        // Mutex poisoning indicates a bug in the calling code, not a recoverable error.
        #[allow(clippy::expect_used)]
        // LCOV_EXCL_LINE defensive: mutex-poisoning — poisoning indicates a bug in the calling code
        let known_dimensions = *self.dimensions.lock().expect("lock poisoned");
        if let Some(dimensions) = known_dimensions {
            let value = format!("{dimensions:>DIM_FIELD_WIDTH$}");
            patch_header_field(
                file,
                &head,
                b"\"dimensions\": ",
                "dimensions",
                &value,
                DIM_FIELD_WIDTH,
            )?;
        }

        file.seek(SeekFrom::End(0))?;
        Ok(())
    }
}

/// Read up to [`HEADER_SCAN_BYTES`] leading bytes from the file.
///
/// Seeks to the start first. Returns fewer bytes when the file is shorter
/// than the scan window.
fn read_header_bytes(file: &mut File) -> ExportResult<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut head = vec![0_u8; HEADER_SCAN_BYTES];
    let mut filled = 0;
    while filled < head.len() {
        let read = file.read(&mut head[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    head.truncate(filled);
    Ok(head)
}

/// Find the byte offset of `needle` inside `haystack`.
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Parse the right-aligned `total_documents` window into a count.
fn parse_count_field(field: &[u8]) -> ExportResult<usize> {
    let text = std::str::from_utf8(field).map_err(|_| {
        ExporterError::WriteError(
            "vector export total_documents field is not valid UTF-8".to_string(),
        )
    })?;
    text.trim().parse::<usize>().map_err(|_| {
        ExporterError::WriteError(format!(
            "vector export total_documents field holds an unparsable count: {text:?}"
        ))
    })
}

/// Count document lines by scanning the whole file for 0x0A bytes.
///
/// The header carries no newline and every document is written with a single
/// `writeln!` (serde_json escapes line breaks inside strings), so in a file
/// truncated at its closing `]` the newline total equals the document count.
fn count_documents(file: &mut File) -> ExportResult<usize> {
    file.seek(SeekFrom::Start(0))?;
    let mut newlines = 0_usize;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        newlines += buffer[..read].iter().filter(|&&byte| byte == b'\n').count();
    }
    Ok(newlines)
}

/// Extract the `created_at` value from a header scan, if present.
fn extract_created_at(head: &[u8]) -> Option<String> {
    let marker = b"\"created_at\": \"";
    let start = find_subslice(head, marker)? + marker.len();
    let end = head[start..].iter().position(|&byte| byte == b'"')? + start;
    std::str::from_utf8(&head[start..end])
        .ok()
        .map(str::to_string)
}

/// Overwrite one reserved header window in place.
///
/// Verifies the sentinel byte (the ',' that must follow the fixed-width
/// window) before writing, so any layout mismatch is an error instead of a
/// silent corruption. Seeks `file` to the window but does not restore the
/// offset afterwards.
fn patch_header_field(
    file: &mut File,
    head: &[u8],
    marker: &[u8],
    field_name: &str,
    value: &str,
    width: usize,
) -> ExportResult<()> {
    if value.len() != width {
        return Err(ExporterError::WriteError(format!(
            "vector export {field_name} value does not fit the reserved {width}-byte window"
        )));
    }
    let value_start = find_subslice(head, marker)
        .map(|pos| pos + marker.len())
        .ok_or_else(|| {
            ExporterError::WriteError(format!(
                "vector export header is missing the {field_name} field"
            ))
        })?;
    if head.get(value_start + width) != Some(&b',') {
        return Err(ExporterError::WriteError(format!(
            "vector export header layout mismatch around {field_name}"
        )));
    }
    file.seek(SeekFrom::Start(value_start as u64))?;
    file.write_all(value.as_bytes())?;
    Ok(())
}

impl Exporter for VectorExporter {
    fn export(&self, document: DocumentChunkValidated) -> ExportResult<()> {
        let (mut file, mut writer) = self.writer()?;
        let is_first_doc =
            !self.config.append || file.metadata().map(|m| m.len() == 0).unwrap_or(true);
        let existing = if is_first_doc {
            0
        } else {
            self.read_append_state(&mut file)?
        };

        self.write_metadata_header(&mut writer, &mut file, is_first_doc)?;

        let serialized = self.serialize_document(&document)?;

        if !is_first_doc {
            write!(writer, ",")?;
        }
        writeln!(writer, "{serialized}")?;

        self.close_json(&mut writer, &mut file, existing + 1)?;

        // Release lock
        fs2::FileExt::unlock(&file)?;

        Ok(())
    }

    #[tracing::instrument(skip(self, documents), fields(exporter = "vector", documents = documents.len()))]
    fn export_batch(&self, documents: &[DocumentChunkValidated]) -> ExportResult<()> {
        if documents.is_empty() {
            return Ok(());
        }

        let (mut file, mut writer) = self.writer()?;
        let is_first_doc =
            !self.config.append || file.metadata().map(|m| m.len() == 0).unwrap_or(true);
        let existing = if is_first_doc {
            0
        } else {
            self.read_append_state(&mut file)?
        };

        self.write_metadata_header(&mut writer, &mut file, is_first_doc)?;

        let mut doc_count = 0;
        for (i, doc) in documents.iter().enumerate() {
            if i > 0 || !is_first_doc {
                write!(writer, ",")?;
            }

            let serialized = self.serialize_document(doc)?;
            writeln!(writer, "{serialized}")?;
            doc_count += 1;
        }

        self.close_json(&mut writer, &mut file, existing + doc_count)?;

        // Release lock
        fs2::FileExt::unlock(&file)?;

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
    use crate::domain::config::ExportFormat;

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

    fn create_test_chunk() -> DocumentChunkValidated {
        use crate::domain::Draft;
        // Create DocumentChunk via From<ScrapedContent> then validate
        let scraped = crate::domain::ScrapedContent {
            title: "Test Document".to_string(),
            content: "Test content for vector export".to_string(),
            url: crate::domain::ValidUrl::parse("https://example.com/test").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        };
        let chunk = crate::domain::DocumentChunk::<Draft>::from(scraped);
        chunk.validate().unwrap()
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

        // Create document and manually add embeddings
        let mut doc = create_test_chunk();
        doc.embeddings = Some(vec![0.1, 0.2, 0.3, 0.4]); // Add embeddings

        let result = exporter.serialize_document(&doc);
        assert!(result.is_ok());

        let json_str = result.unwrap();
        // embeddings field present because we added it
        assert!(
            json_str.contains("embeddings"),
            "expected embeddings field when embeddings is Some"
        );
        assert!(json_str.contains("Test Document"));
    }

    #[test]
    fn test_serialize_document_dimension_mismatch() {
        let config = create_test_config();
        let exporter = VectorExporter::new(config);

        // First document sets dimensions
        let mut doc1 = create_test_chunk();
        doc1.embeddings = Some(vec![0.1, 0.2, 0.3, 0.4]); // 4 dimensions
        let _ = exporter.serialize_document(&doc1);

        // Second document with different dimensions - should warn and serialize without embeddings
        let mut doc2 = create_test_chunk();
        doc2.embeddings = Some(vec![0.1, 0.2]); // Only 2 dimensions

        let result = exporter.serialize_document(&doc2);
        assert!(
            result.is_ok(),
            "dimension mismatch should serialize without embeddings, got: {result:?}"
        );

        let json_str = result.unwrap();
        // Should serialize without embeddings (not an error)
        assert!(
            !json_str.contains("\"embeddings\""),
            "embeddings should be null/absent in output when dimension mismatch"
        );
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
        assert!(
            result.is_ok(),
            "first batch should succeed: {:?}",
            result.err()
        );

        let file1_path = temp_dir.join("test_export.json");
        assert!(
            file1_path.exists(),
            "output file should exist after first batch"
        );

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
        assert_eq!(
            final_doc_count, 3,
            "should have 3 documents after append (2 + 1)"
        );

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
        assert!(
            result.is_err(),
            "export to /root should fail with directory creation error"
        );
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

    // ============================================================================
    // Error path tests
    // ============================================================================

    #[test]
    fn test_export_batch_vs_individual_consistency() {
        let temp_dir = std::env::temp_dir().join("test_batch_vs_individual");
        let _ = std::fs::remove_dir_all(&temp_dir);

        // Export as batch
        let config_batch = create_test_config_with_dir(temp_dir.join("batch"));
        let exporter_batch = VectorExporter::new(config_batch);
        let chunks = vec![create_test_chunk(), create_test_chunk()];
        exporter_batch.export_batch(&chunks).unwrap();

        // Export individually with append mode
        let mut config_ind = create_test_config_with_dir(temp_dir.join("individual"));
        config_ind.append = true;
        let exporter_ind = VectorExporter::new(config_ind);
        let chunk1 = create_test_chunk();
        let chunk2 = create_test_chunk();
        exporter_ind.export(chunk1).unwrap();
        exporter_ind.export(chunk2).unwrap();

        // Both should produce valid JSON
        let individual_path = temp_dir.join("individual/test_export.json");
        let batch_path = temp_dir.join("batch/test_export.json");

        let individual_content = std::fs::read_to_string(&individual_path).unwrap();
        let batch_content = std::fs::read_to_string(&batch_path).unwrap();

        let individual_json: serde_json::Value = serde_json::from_str(&individual_content).unwrap();
        let batch_json: serde_json::Value = serde_json::from_str(&batch_content).unwrap();

        // Both should have 2 documents
        let individual_docs = individual_json["documents"].as_array().unwrap();
        let batch_docs = batch_json["documents"].as_array().unwrap();
        assert!(!individual_docs.is_empty());
        assert!(!batch_docs.is_empty());
        assert_eq!(individual_docs.len(), 2);
        assert_eq!(batch_docs.len(), 2);

        // Both should have the same metadata structure
        assert!(individual_json.get("format_version").is_some());
        assert!(batch_json.get("format_version").is_some());
        assert_eq!(
            individual_json["format_version"],
            batch_json["format_version"]
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_export_append_to_existing_file() {
        let temp_dir = std::env::temp_dir().join("test_append_existing");
        let _ = std::fs::remove_dir_all(&temp_dir);

        // First write without append
        let mut config1 = create_test_config_with_dir(temp_dir.clone());
        config1.append = false;
        let exporter1 = VectorExporter::new(config1);
        let first_chunk = create_test_chunk();
        exporter1.export(first_chunk).unwrap();

        // Read initial content
        let output_path = temp_dir.join("test_export.json");
        let initial_content = std::fs::read_to_string(&output_path).unwrap();
        let initial_json: serde_json::Value = serde_json::from_str(&initial_content).unwrap();
        let docs = initial_json["documents"].as_array().unwrap();
        assert!(!docs.is_empty());
        assert_eq!(docs.len(), 1);

        // Second write with append
        let mut config2 = create_test_config_with_dir(temp_dir.clone());
        config2.append = true;
        let exporter2 = VectorExporter::new(config2);
        let second_chunk = create_test_chunk();
        exporter2.export(second_chunk).unwrap();

        // Read final content
        let final_content = std::fs::read_to_string(&output_path).unwrap();
        let final_json: serde_json::Value = serde_json::from_str(&final_content).unwrap();
        assert_eq!(
            final_json["documents"].as_array().unwrap().len(),
            2,
            "should have 2 docs after append"
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // ============================================================================
    // Issue #502: header total_documents must match the documents array
    // ============================================================================

    /// Parse `<dir>/test_export.json` as JSON.
    fn read_export_json(dir: &std::path::Path) -> serde_json::Value {
        let path = dir.join("test_export.json");
        let content = std::fs::read_to_string(&path).expect("export file should exist");
        serde_json::from_str(&content).expect("export must be valid JSON")
    }

    /// ExporterConfig with append=true — the exact production shape
    /// (`create_exporter` builds every exporter with `.with_append(true)`).
    fn append_config(dir: PathBuf) -> ExporterConfig {
        create_test_config_with_dir(dir).with_append(true)
    }

    /// Issue #502 repro: production always uses append mode, so a fresh
    /// export must still report the real document count in the header.
    #[test]
    fn test_fresh_file_in_append_mode_reports_total_documents() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        let exporter = VectorExporter::new(append_config(temp_dir.path().to_path_buf()));

        let docs = vec![
            create_test_chunk(),
            create_test_chunk(),
            create_test_chunk(),
        ];
        exporter
            .export_batch(&docs)
            .expect("batch export should succeed");

        let json = read_export_json(temp_dir.path());
        let documents = json["documents"]
            .as_array()
            .expect("documents must be an array");
        assert_eq!(documents.len(), 3, "all 3 documents must be present");
        assert_eq!(
            json["total_documents"].as_u64(),
            Some(3),
            "total_documents must equal the documents array length"
        );
    }

    #[test]
    fn test_cross_run_appends_accumulate_total_documents() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        let config = append_config(temp_dir.path().to_path_buf());

        let first_run = VectorExporter::new(config.clone());
        first_run
            .export_batch(&[create_test_chunk(), create_test_chunk()])
            .expect("first batch should succeed");

        let second_run = VectorExporter::new(config);
        second_run
            .export_batch(&[create_test_chunk(), create_test_chunk()])
            .expect("second batch should succeed");

        let json = read_export_json(temp_dir.path());
        assert_eq!(
            json["total_documents"].as_u64(),
            Some(4),
            "total must accumulate across runs"
        );
        assert_eq!(json["documents"].as_array().map(Vec::len), Some(4));
    }

    #[test]
    fn test_append_single_export_after_batch_accumulates_total() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        let config = append_config(temp_dir.path().to_path_buf());

        let first_run = VectorExporter::new(config.clone());
        first_run
            .export_batch(&[create_test_chunk(), create_test_chunk()])
            .expect("batch should succeed");

        let second_run = VectorExporter::new(config);
        second_run
            .export(create_test_chunk())
            .expect("single export should succeed");

        let json = read_export_json(temp_dir.path());
        assert_eq!(
            json["total_documents"].as_u64(),
            Some(3),
            "total must accumulate across runs"
        );
        assert_eq!(json["documents"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn test_legacy_header_is_migrated_on_append() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        let path = temp_dir.path().join("test_export.json");

        let doc1 = serde_json::to_string(&create_test_chunk()).expect("serialize doc1");
        let doc2 = serde_json::to_string(&create_test_chunk()).expect("serialize doc2");
        // Exact shape written by the pre-fix exporter: hardcoded count, no
        // reserved windows, one line per document, closing bracket.
        let legacy = format!(
            "{{\"format_version\": \"1.0\", \"model_name\": null, \"dimensions\": null, \
             \"total_documents\": 0, \"created_at\": \"2026-01-01T00:00:00+00:00\", \
             \"documents\": [{doc1}\n,{doc2}\n]}}\n"
        );
        std::fs::write(&path, legacy).expect("legacy file should be written");

        let exporter = VectorExporter::new(append_config(temp_dir.path().to_path_buf()));
        exporter
            .export(create_test_chunk())
            .expect("append to legacy file should succeed");

        let content = std::fs::read_to_string(&path).expect("read migrated file");
        let json: serde_json::Value =
            serde_json::from_str(&content).expect("migrated file must be valid JSON");
        assert_eq!(
            json["total_documents"].as_u64(),
            Some(3),
            "migrated header must report the real document count"
        );
        assert_eq!(json["documents"].as_array().map(Vec::len), Some(3));
        assert_eq!(
            json["created_at"].as_str(),
            Some("2026-01-01T00:00:00+00:00"),
            "migration must preserve the original created_at"
        );
    }

    #[test]
    fn test_dimensions_are_learned_and_written_to_header() {
        let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
        let exporter = VectorExporter::new(append_config(temp_dir.path().to_path_buf()));

        let mut doc = create_test_chunk();
        doc.embeddings = Some(vec![0.1, 0.2, 0.3, 0.4]);
        exporter
            .export_batch(&[doc])
            .expect("batch with embeddings should succeed");

        let json = read_export_json(temp_dir.path());
        assert_eq!(
            json["dimensions"].as_u64(),
            Some(4),
            "header dimensions must reflect the learned embedding size"
        );
    }
}
