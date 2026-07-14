//! Rust Scraper AI — ONNX-based semantic cleaning
//!
//! Provides AI-powered content cleaning using sentence-transformers models.
//! Depends on `rust_scraper_core` for domain types.

pub mod infrastructure_ai;

// Re-export key types from core
pub use rust_scraper_core::domain::semantic_cleaner::SemanticCleaner;
pub use rust_scraper_core::domain::DocumentChunk;
pub use rust_scraper_core::error::SemanticError;

// Re-export key AI types for convenience
pub use infrastructure_ai::{
    CacheConfig, ChunkId, ContentPruner, HtmlChunker, InferenceEngine, LegibleContentPruner,
    MiniLmTokenizer, ModelCache, ModelConfig, ModelDownloader, RelevanceScorer,
    SemanticCleanerImpl, SentenceSplitter, ThresholdConfig, TokenBatch,
};
