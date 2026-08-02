#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
//! WebFang AI — ONNX-based semantic cleaning
//!
//! Provides AI-powered content cleaning using sentence-transformers models.
//! Depends on `webfang_core` for domain types.
//!
//! The ONNX inference stack is gated behind the `ai` feature; without it the
//! crate only re-exports the core domain types.

#![deny(missing_docs)]

#[cfg(feature = "ai")]
pub mod infrastructure_ai;

// Re-export key types from core
pub use webfang_core::domain::semantic_cleaner::SemanticCleaner;
pub use webfang_core::domain::DocumentChunk;
pub use webfang_core::error::SemanticError;

// Re-export key AI types for convenience
#[cfg(feature = "ai")]
pub use infrastructure_ai::{
    AiModel, ChunkId, ContentPruner, EmbeddingAdapter, HtmlChunker, InferencePool,
    LegibleContentPruner, MarkdownChunker, MiniLmTokenizer, ModelConfig, RelevanceScorer,
    SemanticCleanerImpl, SentenceSplitter, ThresholdConfig, TokenBatch,
};
