//! Exporter trait and configuration for RAG pipeline
//!
//! Defines the interface for exporting scraped content to various formats
//! suitable for retrieval-augmented generation systems.
//!
//! The persistence ports and record DTOs moved to [`crate::domain::persistence`]
//! (ADR-0014, Decision 1); this module re-exports them below so the merged
//! public paths (ADR-0012-B 3.H, ADR-0013 consumers) keep compiling for at
//! least one release cycle.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::crawler_port::filename::confine_filename_component;
use crate::domain::entities::{DocumentChunkUnvalidated, DocumentChunkValidated, ExportFormat};

/// Errors that can occur during export operations
#[derive(Error, Debug)]
pub enum ExporterError {
    /// Failed to create output directory
    #[error("failed to create output directory: {0}")]
    DirectoryCreation(#[from] std::io::Error),

    /// Failed to open or write to file
    #[error("write error: {0}")]
    WriteError(String),

    /// Invalid configuration
    #[error("invalid config: {0}")]
    InvalidConfig(String),

    /// Serialization failed
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Batch operation failed (partial success)
    #[error("batch error: {0}")]
    BatchError(String),

    /// Unsupported export format
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),

    /// State store operation failed
    #[error("state store error: {0}")]
    StateStore(#[from] crate::error::ScraperError),

    /// Embedding dimensions don't match expected size
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    #[allow(missing_docs)] // enum variant fields can't have pub(crate) visibility
    DimensionMismatch { expected: usize, actual: usize },
}

/// Result type for exporter operations
pub type ExportResult<T> = std::result::Result<T, ExporterError>;

/// Configuration for exporter instances
///
/// Contains all settings needed to configure an exporter for a specific format
/// and output location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExporterConfig {
    /// Output directory where files will be written
    pub output_dir: PathBuf,
    /// Export format to use
    pub format: ExportFormat,
    /// Base filename (without extension)
    pub filename: String,
    /// Whether to append to existing files or overwrite
    pub append: bool,
    /// Optional batch size for batch operations
    pub batch_size: Option<usize>,
}

impl ExporterConfig {
    /// Create a new ExporterConfig with required fields.
    ///
    /// The `filename` is confined to a single safe path component (#1125):
    /// `../escape` becomes `export`. Use [`ExporterConfig::output_path`]
    /// for the contained join.
    ///
    /// # Errors
    /// Returns InvalidConfig if output_dir is not a valid directory path
    pub fn new(output_dir: PathBuf, format: ExportFormat, filename: impl Into<String>) -> Self {
        Self {
            output_dir,
            format,
            filename: confine_filename_component(&filename.into(), "export"),
            append: false,
            batch_size: None,
        }
    }

    /// Set append mode
    #[must_use]
    pub fn with_append(mut self, append: bool) -> Self {
        self.append = append;
        self
    }

    /// Set batch size
    #[must_use]
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = Some(size);
        self
    }

    /// Get the full output file path.
    ///
    /// The filename is confined at join time (#1125), so even a struct
    /// literal with a hostile `filename` cannot escape `output_dir`.
    #[must_use]
    pub fn output_path(&self) -> PathBuf {
        let ext = self.format.extension();
        let safe = confine_filename_component(&self.filename, "export");
        self.output_dir.join(format!("{safe}.{ext}"))
    }

    /// Get the state file path for this configuration.
    ///
    /// Same join-time confinement as [`ExporterConfig::output_path`].
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        let state_dir = self.output_dir.join("state");
        // Extract domain from filename if possible, otherwise use filename
        let domain = confine_filename_component(&self.filename, "export");
        state_dir.join(format!("{domain}.json"))
    }
}

/// Default implementation for ExporterConfig
impl Default for ExporterConfig {
    fn default() -> Self {
        Self {
            output_dir: PathBuf::from("./output"),
            format: ExportFormat::Jsonl,
            filename: "export".to_string(),
            append: false,
            batch_size: None,
        }
    }
}

/// Trait for exporting document chunks to various formats
///
/// Implementors must provide:
/// - Synchronous export (export method)
/// - Batch export (export_batch method)
///
/// The trait is designed to be:
/// - `Sync`: Safe to share across threads
/// - `'static`: No lifetime dependencies on caller
///
/// # Example
/// ```ignore
/// struct JsonlExporter {
///     config: ExporterConfig,
/// }
///
/// impl Exporter for JsonlExporter {
///     fn export(&self, document: DocumentChunk<Validated>) -> ExportResult<()> { ... }
///     fn export_batch(&self, documents: &[DocumentChunk<Validated>]) -> ExportResult<()> { ... }
/// }
/// ```
pub trait Exporter: Send + Sync + 'static {
    /// Export a single document chunk (must be Validated state)
    ///
    /// # Arguments
    /// * `document` - The document chunk in Validated state to export
    ///
    /// # Errors
    /// Returns ExporterError if export fails
    fn export(&self, document: DocumentChunkValidated) -> ExportResult<()>;

    /// Export multiple documents in batch (must be Validated state)
    ///
    /// This method is optimized for bulk operations and may:
    /// - Batch I/O operations for better performance
    /// - Use streaming writes for large datasets
    /// - Maintain transaction semantics
    ///
    /// # Arguments
    /// * `documents` - Slice of document chunks in Validated state to export
    ///
    /// # Errors
    /// Returns ExporterError if any document fails to export
    fn export_batch(&self, documents: &[DocumentChunkValidated]) -> ExportResult<()>;

    /// Get the configuration for this exporter
    fn config(&self) -> &ExporterConfig;

    /// Get the format this exporter produces
    fn format(&self) -> ExportFormat {
        self.config().format
    }
}

/// Extension trait for convenient exporter operations
pub trait ExporterExt: Exporter {
    /// Export a single document, converting from ScrapedContent
    ///
    /// Convenience method that handles the conversion from ScrapedContent
    /// to DocumentChunk and validates it internally.
    fn export_scraped(&self, scraped: &crate::domain::ScrapedContent) -> ExportResult<()> {
        let chunk = DocumentChunkUnvalidated::from_scraped_content(scraped);
        let validated = chunk
            .validate()
            .map_err(|e| ExporterError::InvalidConfig(e.to_string()))?;
        self.export(validated)
    }

    /// Export multiple scraped contents in batch
    fn export_scraped_batch(
        &self,
        scraped_contents: &[crate::domain::ScrapedContent],
    ) -> ExportResult<()> {
        let chunks: Vec<DocumentChunkValidated> = scraped_contents
            .iter()
            .map(|s| {
                let chunk = DocumentChunkUnvalidated::from_scraped_content(s);
                chunk
                    .validate()
                    .map_err(|e| ExporterError::InvalidConfig(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.export_batch(&chunks)
    }

    /// Check if the exporter is configured to append
    fn is_append_mode(&self) -> bool {
        self.config().append
    }

    /// Get the output path
    fn output_path(&self) -> PathBuf {
        self.config().output_path()
    }
}

impl<T: Exporter> ExporterExt for T {}

// ---------------------------------------------------------------------------
// Persistence ports + record DTOs — re-exports (ADR-0014, Decision 1)
//
// The canonical definitions now live in `crate::domain::persistence`. These
// `pub use` re-exports keep the old public paths valid for at least one
// release cycle (ADR-0014 consequence: churn paid once, merged ADR-0012-B
// 3.H / ADR-0013 consumers keep compiling).
// ---------------------------------------------------------------------------

pub use crate::domain::persistence::{
    DomainRecords, LastError, RawRecord, RecordStoreError, RecordStorePort, StateStorePort,
};

#[cfg(test)]
#[allow(clippy::io_other_error)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_export_format_extension() {
        // ExportFormat is for RAG pipeline: Jsonl, Auto
        assert_eq!(ExportFormat::Jsonl.extension(), "jsonl");
        assert_eq!(ExportFormat::Auto.extension(), "auto");
    }

    #[test]
    fn test_export_format_name() {
        // ExportFormat is for RAG pipeline: Jsonl, Auto
        assert_eq!(ExportFormat::Jsonl.name(), "JSONL");
        assert_eq!(ExportFormat::Auto.name(), "Auto");
    }

    #[test]
    fn test_exporter_config_default() {
        let config = ExporterConfig::default();
        assert_eq!(config.format, ExportFormat::Jsonl);
        assert_eq!(config.filename, "export");
        assert!(!config.append);
    }

    #[test]
    fn test_exporter_config_output_path() {
        let config = ExporterConfig::new(
            PathBuf::from("/tmp/output"),
            ExportFormat::Jsonl,
            "test_export",
        );
        assert_eq!(
            config.output_path(),
            PathBuf::from("/tmp/output/test_export.jsonl")
        );
    }

    #[test]
    fn hostile_filename_is_confined_inside_output_dir() {
        // Issue #1125 proof vectors: hostile filenames collapse to a single
        // safe component; the join can never escape `output_dir`, whether
        // built through `new` or through a struct literal (pub fields).
        for hostile in ["../escape", "sub/out", "..\\escape", ".."] {
            let config =
                ExporterConfig::new(PathBuf::from("/tmp/output"), ExportFormat::Jsonl, hostile);
            let path = config.output_path();
            assert!(
                path.starts_with("/tmp/output"),
                "{hostile:?} escaped: {}",
                path.display()
            );
            assert_eq!(path.parent(), Some(PathBuf::from("/tmp/output").as_path()));

            let literal = ExporterConfig {
                output_dir: PathBuf::from("/tmp/output"),
                format: ExportFormat::Jsonl,
                filename: hostile.to_string(),
                append: false,
                batch_size: None,
            };
            let literal_path = literal.output_path();
            assert_eq!(
                literal_path.parent(),
                Some(PathBuf::from("/tmp/output").as_path()),
                "struct literal {hostile:?} escaped"
            );
            let literal_state = literal.state_path();
            assert_eq!(
                literal_state.parent(),
                Some(PathBuf::from("/tmp/output/state").as_path()),
                "struct literal state {hostile:?} escaped"
            );
        }
    }

    #[test]
    fn test_exporter_config_with_builder_pattern() {
        let config = ExporterConfig::new(PathBuf::from("/data"), ExportFormat::Jsonl, "my_data")
            .with_append(true)
            .with_batch_size(1000);

        assert_eq!(config.output_dir, PathBuf::from("/data"));
        assert_eq!(config.format, ExportFormat::Jsonl);
        assert_eq!(config.filename, "my_data");
        assert!(config.append);
        assert_eq!(config.batch_size, Some(1000));
    }

    #[test]
    fn test_exporter_error_messages() {
        let io_error = std::io::Error::new(std::io::ErrorKind::Other, "path error");
        let err = ExporterError::DirectoryCreation(io_error);
        assert!(err
            .to_string()
            .contains("failed to create output directory"));

        let err = ExporterError::WriteError("disk full".to_string());
        assert!(err.to_string().contains("write error: disk full"));

        let err = ExporterError::InvalidConfig("missing path".to_string());
        assert!(err.to_string().contains("invalid config"));
    }
}
