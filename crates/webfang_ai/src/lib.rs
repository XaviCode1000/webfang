//! WebFang AI — ONNX-based semantic cleaning
//!
//! Provides AI-powered content cleaning using sentence-transformers models.
//! Depends on `webfang_core` for domain types.

pub mod infrastructure_ai;

// Re-export key types from core
pub use webfang_core::domain::semantic_cleaner::SemanticCleaner;
pub use webfang_core::domain::DocumentChunk;
pub use webfang_core::error::SemanticError;

// Re-export key AI types for convenience
pub use infrastructure_ai::{
    AiModel, CacheConfig, ChunkId, ContentPruner, HtmlChunker, InferencePool,
    LegibleContentPruner, MiniLmTokenizer, ModelCache, ModelConfig, ModelDownloader,
    RelevanceScorer, SemanticCleanerImpl, SentenceSplitter, ThresholdConfig, TokenBatch,
};
