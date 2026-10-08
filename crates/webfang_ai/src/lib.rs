#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
//! WebFang AI — ONNX-based semantic cleaning
//!
//! Provides AI-powered content cleaning using sentence-transformers models.
//! Depends on `webfang_core` for domain types.
//!
//! The ONNX inference stack is gated behind the `ai` feature. Without it the
//! crate still provides the text half of the pipeline (chunker, sentence
//! splitter, relevance scorer, content pruner, threshold config) plus the core
//! domain re-exports — that half carries no ONNX dependency and exists so the
//! `EmbeddingPort` seam is usable with no local model.

#![deny(missing_docs)]

pub mod infrastructure_ai;

// Re-export key types from core
pub use webfang_core::domain::semantic_cleaner::SemanticCleaner;
pub use webfang_core::domain::DocumentChunk;
pub use webfang_core::error::SemanticError;

// Text pipeline — no ONNX dependency, available without the `ai` feature.
pub use infrastructure_ai::{
    ChunkId, ContentPruner, HtmlChunker, LegibleContentPruner, MarkdownChunker, RelevanceScorer,
    SentenceSplitter, ThresholdConfig,
};

// Re-export key AI types for convenience
#[cfg(feature = "ai")]
pub use infrastructure_ai::{
    AiModel, EmbeddingAdapter, GraniteDomInspector, InferencePool, MiniLmTokenizer, ModelConfig,
    SemanticCleanerImpl, TokenBatch,
};
