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
pub use file_exporter::FileExporter;
pub use jsonl_exporter::JsonlExporter;
pub use jsonl_writer::JsonlSession;
pub use record_store::RecordStore;
pub use state_store::StateStore;
pub use vector_exporter::VectorExporter;
// Record DTOs + error moved to `domain::exporter` (ADR-0012-B 3.H); the
// infra paths below keep resolving during the shim window.
pub use crate::domain::exporter::{DomainRecords, LastError, RawRecord, RecordStoreError};
