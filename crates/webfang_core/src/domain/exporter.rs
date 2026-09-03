//! Exporter trait and configuration for RAG pipeline
//!
//! Defines the interface for exporting scraped content to various formats
//! suitable for retrieval-augmented generation systems.
//!
//! Also hosts the resume/record port (ADR-0012-B 3.H): the persisted per-URL
//! record DTOs ([`RawRecord`], [`LastError`], [`DomainRecords`]) and the
//! [`RecordStorePort`] seam. The JSON-per-domain concrete store stays in
//! `infrastructure::export::record_store` and implements the port here.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::domain::entities::{
    DocumentChunkUnvalidated, DocumentChunkValidated, ExportFormat, ExportState,
};
use crate::domain::error::ErrorClass;
use crate::domain::page_state::{PageStatus, PersistedRecord, MIGRATED_V1_RUN_ID};

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
// Resume record port (ADR-0012-B 3.H)
//
// The persisted per-URL record DTOs live in the domain because both the
// application layer (`export_factory`, `resume`) and the infra store mutate
// them through the typestate lifecycle. The JSON-per-domain store itself
// stays in `infrastructure::export::record_store` and implements the port
// below (WafInspectorPort pattern, #993): concrete in infra, seam in domain.
// ---------------------------------------------------------------------------

/// Keyed by canonical URL; deterministic serialization order.
pub type DomainRecords = BTreeMap<String, RawRecord>;

/// A classified error attached to a persisted record (SC6 taxonomy).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    /// Operational classification of the failure.
    pub class: ErrorClass,
    /// Human-readable failure message.
    pub message: String,
}

/// The persisted per-URL record — exactly the nine spec fields, no more,
/// no less (frozen contract; unknown fields are rejected on load).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRecord {
    /// Original URL as discovered.
    pub url: String,
    /// Resolved canonical URL; www/apex unify here.
    pub canonical_url: String,
    /// uuid v4 of the run that last touched this record; `"migrated-v1"`
    /// for migrated legacy entries.
    pub run_id: String,
    /// Hash of the serialized payload; dedup/reconciliation key (D3).
    pub content_hash: Option<String>,
    /// Persisted attempt count. NOT actuated — retry policy is a non-goal.
    pub attempts: u32,
    /// Lifecycle position (the ONLY machine state that serializes).
    pub status: PageStatus,
    /// Last classified failure, cleared on success.
    pub last_error: Option<LastError>,
    /// Output path recorded at the EXPORTED checkpoint.
    pub output_location: Option<String>,
    /// Unix millis UTC of the last persist.
    pub updated_at: i64,
}

impl RawRecord {
    /// Validated constructor for a fresh DISCOVERED lifecycle (#876).
    ///
    /// This is the choke point every writer of new records MUST pass through:
    /// an empty or whitespace-only URL is structurally meaningless identity
    /// and is rejected with [`RecordStoreError::InvalidRecord`] instead of
    /// ever becoming retrievable state. Fields default to the honest fresh
    /// shape (`attempts = 0`, no hash, no output location, no error); callers
    /// mutate from there via the typestate lifecycle.
    ///
    /// # Errors
    ///
    /// [`RecordStoreError::InvalidRecord`] when `url` or `canonical_url` is
    /// empty after trimming.
    pub fn new_discovered(
        url: &str,
        canonical_url: &str,
        run_id: &str,
        updated_at: i64,
    ) -> Result<Self, RecordStoreError> {
        if url.trim().is_empty() {
            return Err(RecordStoreError::InvalidRecord {
                reason: "url must not be empty",
            });
        }
        if canonical_url.trim().is_empty() {
            return Err(RecordStoreError::InvalidRecord {
                reason: "canonical_url must not be empty",
            });
        }
        Ok(Self {
            url: url.to_string(),
            canonical_url: canonical_url.to_string(),
            run_id: run_id.to_string(),
            content_hash: None,
            attempts: 0,
            status: PageStatus::Discovered,
            last_error: None,
            output_location: None,
            updated_at,
        })
    }
}

impl PersistedRecord for RawRecord {
    fn status(&self) -> PageStatus {
        self.status
    }

    fn output_location(&self) -> Option<&str> {
        self.output_location.as_deref()
    }

    fn content_hash(&self) -> Option<&str> {
        self.content_hash.as_deref()
    }

    fn has_last_error(&self) -> bool {
        self.last_error.is_some()
    }

    fn attempts(&self) -> u32 {
        self.attempts
    }

    fn set_status(&mut self, status: PageStatus) {
        self.status = status;
    }

    fn is_migrated_v1(&self) -> bool {
        self.run_id == MIGRATED_V1_RUN_ID
    }
}

/// Typed failures of the record store. Callers apply the named-path
/// fresh-start policy (`load_or_init`) on [`RecordStoreError::Corrupt`] and
/// [`RecordStoreError::UnsupportedVersion`] — never silently (Gate 2).
#[derive(Debug, thiserror::Error)]
pub enum RecordStoreError {
    /// Filesystem failure at a known path.
    #[error("record store I/O error at {path}: {source}")]
    Io {
        /// Path the failing operation targeted.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// The state file exists but is not valid JSON / not a valid store.
    #[error("record store file is corrupt or not valid JSON: {path}")]
    Corrupt {
        /// Path of the unreadable file.
        path: PathBuf,
    },
    /// The state file carries a version neither 1 nor 2.
    #[error("record store file has unsupported version {found}: {path}")]
    UnsupportedVersion {
        /// Path of the offending file.
        path: PathBuf,
        /// Version found in the envelope.
        found: u32,
    },
    /// Creating the pre-migration backup failed; migration aborted.
    #[error("failed to create v1 backup at {path}: {source}")]
    Backup {
        /// Intended backup path.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// A writer attempted to create a record whose identity is structurally
    /// meaningless (#876): an empty URL can never address a fetched page, so
    /// it is rejected at the domain boundary instead of becoming persisted
    /// state (fail-closed, same class as unparseable legacy input).
    #[error("invalid record rejected at domain boundary: {reason}")]
    InvalidRecord {
        /// The invariant that failed (English, log-oriented).
        reason: &'static str,
    },
}

/// Domain-owned seam over the JSON-per-domain record store (ADR-0012-B 3.H).
///
/// `application` consumes this trait; the concrete
/// `infrastructure::export::record_store::RecordStore` implements it. Object
/// safe: call sites hold `&dyn RecordStorePort`.
pub trait RecordStorePort: Send + Sync {
    /// Persist `records` atomically (temp file + rename(2), no fsync).
    ///
    /// # Errors
    /// [`RecordStoreError`] on filesystem failure or serialization error.
    fn save(&self, records: &DomainRecords) -> Result<(), RecordStoreError>;

    /// Load the persisted records for this domain.
    ///
    /// # Errors
    /// [`RecordStoreError::Corrupt`] / [`RecordStoreError::UnsupportedVersion`]
    /// for unreadable state; callers apply the named-path fresh-start policy.
    fn load(&self) -> Result<DomainRecords, RecordStoreError>;

    /// Load without ever discarding prior history: unreadable files degrade
    /// to an empty in-memory view (the original bytes stay on disk).
    fn load_or_init(&self) -> DomainRecords;
}

/// Domain-owned seam over the legacy JSON-per-domain state store (#1097).
///
/// `cli` constructs the concrete
/// `infrastructure::export::state_store::StateStore` through
/// [`crate::application::container::build_state_store`]; `cli` consumes it
/// through this port only. Seam covers the four state-file methods in live
/// use (`get_state_path`, `load`, `save`, `load_or_default`);
/// `mark_processed`/`is_processed` are excluded — zero production callers.
/// Object safe: call sites hold `&dyn StateStorePort` or
/// `Arc<dyn StateStorePort>`.
///
/// Errors surface as [`crate::error::ScraperError`]; export callers map them
/// into [`ExporterError::StateStore`] via `?` (its first honest use).
pub trait StateStorePort: Send + Sync {
    /// Full path to the domain state JSON file.
    fn get_state_path(&self) -> PathBuf;

    /// Load existing export state from disk.
    ///
    /// # Errors
    /// [`crate::error::ScraperError`] if the file is missing or unparsable.
    fn load(&self) -> crate::error::Result<ExportState>;

    /// Persist export state to disk atomically.
    ///
    /// # Errors
    /// [`crate::error::ScraperError`] on directory, write, or rename failure.
    fn save(&self, state: &ExportState) -> crate::error::Result<()>;

    /// Load existing state or return a fresh one when absent or stale-versioned.
    ///
    /// # Errors
    /// [`crate::error::ScraperError`] on corrupt JSON or non-NotFound I/O.
    fn load_or_default(&self) -> crate::error::Result<ExportState>;
}

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
