#![cfg_attr(not(test), deny(clippy::unwrap_used))]
//! WebFang AI — ONNX-based semantic cleaning
//!
//! Provides AI-powered content cleaning using sentence-transformers models.
//! Depends on `webfang_core` for domain types.

#![deny(missing_docs)]

pub mod infrastructure_ai;

// Re-export key types from core
pub use webfang_core::domain::semantic_cleaner::SemanticCleaner;
pub use webfang_core::domain::DocumentChunk;
pub use webfang_core::error::SemanticError;

// Re-export key AI types for convenience
pub use infrastructure_ai::{
    AiModel, ChunkId, ContentPruner, HtmlChunker, InferencePool, LegibleContentPruner,
    MiniLmTokenizer, ModelConfig, RelevanceScorer, SemanticCleanerImpl, SentenceSplitter,
    ThresholdConfig, TokenBatch,
};
