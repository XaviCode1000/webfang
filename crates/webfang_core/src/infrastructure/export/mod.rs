//! Export pipeline implementations for RAG systems
//!
//! This module contains the concrete implementations of the Exporter trait
//! for different output formats:
//! - JSONL (JSON Lines)
//! - File (Markdown, Text, JSON)
//! - Vector (embeddings for vector databases)
//!
//! Following Clean Architecture: infrastructure depends on domain.

pub mod file_exporter;
pub mod jsonl_exporter;
pub mod jsonl_writer;
pub mod record_store;
pub mod state_store;
pub mod vector_exporter;

// Re-export for convenience
pub use crate::domain::exporter::CHECKSUM_FIELD;
pub use file_exporter::FileExporter;
pub use jsonl_exporter::JsonlExporter;
pub use jsonl_writer::JsonlSession;
pub use record_store::RecordStore;
pub use state_store::StateStore;
pub use vector_exporter::VectorExporter;
// Record DTOs + error moved to `domain::exporter` (ADR-0012-B 3.H); the
// infra paths below keep resolving during the shim window.
pub use crate::domain::exporter::{DomainRecords, LastError, RawRecord, RecordStoreError};

use crate::domain::exporter::ExportResult;

/// Record how an `export_batch` call ended on its own span (#1610, OBS-H6).
///
/// A span that only knows "N documents, M seconds" cannot answer "did the
/// export land, and how much data left?", so both are declared `Empty` and
/// recorded here. `payload_bytes` is `None` for exporters that delegate their
/// writes to a helper reporting no size: an absent key is the honest reading,
/// a made-up number would be indistinguishable from a real one.
pub(crate) fn record_export_outcome(outcome: &str, payload_bytes: Option<u64>) {
    let span = tracing::Span::current();
    span.record("outcome", outcome);
    if let Some(bytes) = payload_bytes {
        span.record("payload_bytes", bytes);
    }
}

/// Run an exporter's batch write loop and record the outcome on the caller's span.
///
/// Shared `export_batch` wrapper for the JSONL and vector exporters (#1880):
/// both serialize every document themselves, so their write loop reports the
/// exact payload byte volume and the wrapper records it identically. The
/// closure runs synchronously inside the caller's instrumented `export_batch`
/// span, so `record_export_outcome` still writes to that span — observability
/// behavior is unchanged, and errors propagate exactly as before.
pub(crate) fn run_export_batch(
    write_batch: impl FnOnce() -> ExportResult<u64>,
) -> ExportResult<()> {
    match write_batch() {
        Ok(payload_bytes) => {
            record_export_outcome("ok", Some(payload_bytes));
            Ok(())
        },
        Err(e) => {
            record_export_outcome("error", None);
            Err(e)
        },
    }
}
