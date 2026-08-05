//! AI module — Full RAG Pipeline Integration (Phase 2 + Phase 3)
//!
//! This module provides AI-powered semantic cleaning capabilities with full pipeline integration:
//! - Model resolution via hf_hub native cache (cache-first offline via `hf_hub::Cache`,
//!   `ApiRepo` online, with in-memory SHA256 validation)
//! - Memory-mapped model loading (zero-copy for HDD optimization)
//! - ONNX inference for embedding generation (Phase 2)
//! - Semantic chunking with arena allocator (Phase 3)
//! - SIMD-accelerated cosine similarity filtering (Phase 3)
//!
//! # Architecture
//!
//! Following Clean Architecture, this module implements the [`SemanticCleaner`](crate::SemanticCleaner)
//! trait defined in the domain layer.
//!
//! ```text
//! HTML Input
//!     ↓
//! [Chunker] Split into semantic chunks (arena allocator)
//!     ↓
//! [Tokenizer] Convert each chunk to token IDs
//!     ↓
//! [InferencePool] Generate embeddings (dedicated worker threads)
//!     ↓
//! [RelevanceScorer] Filter by threshold (SIMD cosine similarity)
//!     ↓
//! Vec<DocumentChunk> Output
//! ```
//!
//! # Features
//!
//! This module is feature-gated behind the `ai` feature flag:
//!
//! ```toml
//! [dependencies]
//! webfang = { version = "1.0", features = ["ai"] }
//! ```
//!
//! # Model Information
//!
//! - **Model**: `sentence-transformers/all-MiniLM-L6-v2`
//! - **Format**: ONNX (optimized for inference)
//! - **Size**: ~90MB
//! - **Max Tokens**: 512 per chunk
//! - **Cache Location**: hf_hub native cache (`~/.cache/huggingface/hub`)
//!
//! # Rust-Skills Applied
//!
//! - `async-join-parallel`: Concurrent embedding generation
//! - `mem-reuse-collections`: Buffer reuse
//! - `own-borrow-over-clone`: Borrow over clone
//! - `async-spawn-blocking`: CPU-intensive inference
//! - `opt-simd-portable`: SIMD cosine similarity
//!
//! # Examples
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use webfang_ai::{SemanticCleaner, SemanticCleanerImpl, ModelConfig};
//!
//! let config = ModelConfig::default();
//! let cleaner = SemanticCleanerImpl::new(config).await?;
//!
//! let html = "<article><p>Hello World</p></article>";
//! let chunks = cleaner.clean("https://example.com", html).await?;
//!
//! println!("Generated {} chunks", chunks.len());
//! # Ok(())
//! # }
//! ```

// Core AI infrastructure (Modules 1-2)
pub mod cache_config;

pub mod semantic_cleaner_impl;

/// Adapter bridging `InferencePool` to the domain `EmbeddingPort` (#433).
pub mod embedding_adapter;

pub mod inference_engine;

pub mod tokenizer;

/// Unique identifier for content chunks with newtype safety.
pub mod chunk_id;

pub mod sentence;

pub mod chunker;

pub mod markdown_chunker;

pub mod embedding_ops;

pub mod relevance_scorer;

pub mod threshold_config;

pub mod content_pruner;

pub mod granite_dom_inspector;

// Re-exports for convenience (Modules 1-2)
pub use cache_config::{
    AiModel, DEFAULT_MODEL_FILE, DEFAULT_MODEL_REPO, DEFAULT_MODEL_SHA256, MODEL_SELECTION_ENV,
};

pub use semantic_cleaner_impl::{ModelConfig, SemanticCleanerImpl};

pub use embedding_adapter::EmbeddingAdapter;

pub use inference_engine::InferencePool;

pub use tokenizer::{MiniLmTokenizer, TokenBatch, DEFAULT_MAX_LENGTH};

pub use inference_engine::ModelInput;

// Re-exports for Semantic Chunking (Modules 3-4)
pub use chunk_id::ChunkId;

pub use sentence::SentenceSplitter;

pub use chunker::HtmlChunker;

pub use markdown_chunker::MarkdownChunker;

pub use relevance_scorer::RelevanceScorer;

pub use threshold_config::ThresholdConfig;

pub use content_pruner::{ContentPruner, LegibleContentPruner, PruneAggressiveness};

pub use granite_dom_inspector::GraniteDomInspector;
