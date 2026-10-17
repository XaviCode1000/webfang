#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
// Explicit per docs-truthfulness audit (S5): these rustdoc lints are
// allow-by-default, so CI's `-D warnings` never reaches them. Declare intent.
#![allow(rustdoc::missing_crate_level_docs)]
#![allow(rustdoc::private_doc_tests)]
#![allow(rustdoc::unescaped_backticks)]
//! WebFang AI — ONNX-based semantic cleaning
//!
//! Provides AI-powered content cleaning using sentence-transformers models.
//! Depends on `webfang_core` for domain types.
//!
//! The ONNX inference stack is gated behind the `ai` feature. Without it the
//! crate still provides the text half of the pipeline (chunker, sentence
//! splitter, relevance scorer, content pruner, threshold config), the core
//! domain re-exports, AND the semantic cleaner itself — that half carries no
//! ONNX dependency and exists so the `EmbeddingPort` seam is usable with no
//! local model, against a remote endpoint or a fake alike.
//!
//! The Tier 2 selector inspector is not part of this crate anymore: it is
//! `webfang_core::infrastructure::scraper::embedding_semantic_inspector::EmbeddingSemanticInspector`
//! (ADR-0004, slice 1), over the same erased port.

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
pub use infrastructure_ai::{AiModel, ModelConfig, SemanticCleanerImpl};

#[cfg(feature = "ai")]
pub use infrastructure_ai::{EmbeddingAdapter, InferencePool, MiniLmTokenizer, TokenBatch};
