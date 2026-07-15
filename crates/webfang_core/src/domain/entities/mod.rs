//! Domain entities — Core business types
//!
//! These are the fundamental data structures used throughout the application.
//! They are serializable for persistence but contain no business logic.

pub mod content;
pub mod download;
pub mod export;

pub use content::{
    DocumentChunk, DocumentChunkExported, DocumentChunkUnvalidated, DocumentChunkValidated, Draft,
    Exported, ScrapedContent, Validated, ValidationError,
};
pub use download::DownloadedAsset;
pub use export::{ExportFormat, ExportState};
