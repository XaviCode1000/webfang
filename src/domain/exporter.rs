//! Exporter trait and configuration for RAG pipeline
//!
//! Defines the interface for exporting scraped content to various formats
//! suitable for retrieval-augmented generation systems.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::entities::{DocumentChunk, ExportFormat};

/// Errors that can occur during export operations
#[derive(Error, Debug)]
pub enum ExporterError {
    /// Failed to create output directory
    #[error("No se pudo crear el directorio de salida: {0}")]
    DirectoryCreation(#[from] std::io::Error),

    /// Failed to open or write to file
    #[error("Error de escritura: {0}")]
    WriteError(String),

    /// Invalid configuration
    #[error("Configuración inválida: {0}")]
    InvalidConfig(String),

    /// Serialization failed
    #[error("Error de serialización: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Batch operation failed (partial success)
    #[error("Error en batch: {0}")]
    BatchError(String),

    /// State store operation failed
    #[error("Error en state store: {0}")]
    StateStore(#[from] crate::error::ScraperError),
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
    /// Create a new ExporterConfig with required fields
    ///
    /// # Errors
    /// Returns InvalidConfig if output_dir is not a valid directory path
    pub fn new(output_dir: PathBuf, format: ExportFormat, filename: impl Into<String>) -> Self {
        Self {
            output_dir,
            format,
            filename: filename.into(),
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

    /// Get the full output file path
    #[must_use]
    pub fn output_path(&self) -> PathBuf {
        let ext = self.format.extension();
        self.output_dir.join(format!("{}.{}", self.filename, ext))
    }

    /// Get the state file path for this configuration
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        let state_dir = self.output_dir.join("state");
        // Extract domain from filename if possible, otherwise use filename
        let domain = self.filename.clone();
        state_dir.join(format!("{}.json", domain))
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
///     fn export(&self, documents: Vec<DocumentChunk>) -> ExportResult<()> { ... }
///     fn export_batch(&self, documents: Vec<DocumentChunk>) -> ExportResult<()> { ... }
/// }
/// ```
pub trait Exporter: Send + Sync + 'static {
    /// Export a single document chunk
    ///
    /// # Arguments
    /// * `document` - The document chunk to export
    ///
    /// # Errors
    /// Returns ExporterError if export fails
    fn export(&self, document: DocumentChunk) -> ExportResult<()>;

    /// Export multiple documents in batch
    ///
    /// This method is optimized for bulk operations and may:
    /// - Batch I/O operations for better performance
    /// - Use streaming writes for large datasets
    /// - Maintain transaction semantics
    ///
    /// # Arguments
    /// * `documents` - Collection of document chunks to export
    ///
    /// # Errors
    /// Returns ExporterError if any document fails to export
    fn export_batch(&self, documents: Vec<DocumentChunk>) -> ExportResult<()>;

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
    /// to DocumentChunk internally.
    fn export_scraped(&self, scraped: &crate::domain::ScrapedContent) -> ExportResult<()> {
        let chunk = DocumentChunk::from_scraped_content(scraped);
        self.export(chunk)
    }

    /// Export multiple scraped contents in batch
    fn export_scraped_batch(
        &self,
        scraped_contents: Vec<crate::domain::ScrapedContent>,
    ) -> ExportResult<()> {
        let chunks: Vec<DocumentChunk> = scraped_contents
            .iter()
            .map(DocumentChunk::from_scraped_content)
            .collect();
        self.export_batch(chunks)
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_export_format_extension() {
        assert_eq!(ExportFormat::Markdown.extension(), "md");
        assert_eq!(ExportFormat::Jsonl.extension(), "jsonl");
        assert_eq!(ExportFormat::Zvec.extension(), "zvec");
    }

    #[test]
    fn test_export_format_name() {
        assert_eq!(ExportFormat::Markdown.name(), "Markdown");
        assert_eq!(ExportFormat::Jsonl.name(), "JSONL");
        assert_eq!(ExportFormat::Zvec.name(), "Zvec");
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
    fn test_exporter_config_with_builder_pattern() {
        let config = ExporterConfig::new(PathBuf::from("/data"), ExportFormat::Zvec, "my_data")
            .with_append(true)
            .with_batch_size(1000);

        assert_eq!(config.output_dir, PathBuf::from("/data"));
        assert_eq!(config.format, ExportFormat::Zvec);
        assert_eq!(config.filename, "my_data");
        assert!(config.append);
        assert_eq!(config.batch_size, Some(1000));
    }

    #[test]
    fn test_exporter_error_messages() {
        let io_error = std::io::Error::new(std::io::ErrorKind::Other, "path error");
        let err = ExporterError::DirectoryCreation(io_error);
        assert!(err.to_string().to_lowercase().contains("directorio"));

        let err = ExporterError::WriteError("disk full".to_string());
        assert!(err.to_string().to_lowercase().contains("escritura"));

        let err = ExporterError::InvalidConfig("missing path".to_string());
        assert!(err.to_string().to_lowercase().contains("inválida"));
    }
}
