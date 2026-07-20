//! AI module — Full RAG Pipeline Integration (Phase 2 + Phase 3)
//!
//! This module provides AI-powered semantic cleaning capabilities with full pipeline integration:
//! - Automatic model download from HuggingFace Hub
//! - Cache management with SHA256 validation
//! - Memory-mapped model loading (zero-copy for HDD optimization)
//! - ONNX inference for embedding generation (Phase 2)
//! - Semantic chunking with arena allocator (Phase 3)
//! - SIMD-accelerated cosine similarity filtering (Phase 3)
//!
//! # Architecture
//!
//! Following Clean Architecture, this module implements the [`SemanticCleaner`](webfang_core::domain::semantic_cleaner::SemanticCleaner)
//! trait defined in the domain layer.
//!
//! ```text
//! HTML Input
//!     ↓
//! [Chunker] Split into semantic chunks (arena allocator)
//!     ↓
//! [Tokenizer] Convert each chunk to token IDs
//!     ↓
//! [InferenceEngine] Generate embeddings (spawn_blocking, concurrent)
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
//! - **Cache Location**: `~/.cache/webfang/ai_models/`
//!
//! # Rust-Skills Applied
//!
//! - [`async-join-parallel`](crate::rust_skills::async_join_parallel): Concurrent embedding generation
//! - [`mem-reuse-collections`](crate::rust_skills::mem_reuse_collections): Buffer reuse
//! - [`own-borrow-over-clone`](crate::rust_skills::own_borrow_over_clone): Borrow over clone
//! - [`async-spawn-blocking`](crate::rust_skills::async_spawn_blocking): CPU-intensive inference
//! - [`opt-simd-portable`](crate::rust_skills::opt_simd_portable): SIMD cosine similarity
//!
//! # Examples
//!
//! ```no_run
//! # async fn example() -> anyhow::Result<()> {
//! use webfang::infrastructure::ai::{SemanticCleanerImpl, ModelConfig};
//!
//! let config = ModelConfig::default();
//! let cleaner = SemanticCleanerImpl::new(config).await?;
//!
//! let html = "<article><p>Hello World</p></article>";
//! let chunks = cleaner.clean(html).await?;
//!
//! println!("Generated {} chunks", chunks.len());
//! # Ok(())
//! # }
//! ```

// Core AI infrastructure (Modules 1-2)
pub mod cache_config;
pub mod model_cache;

pub mod model_downloader;

pub mod semantic_cleaner_impl;

pub mod inference_engine;

pub mod tokenizer;

// Semantic Chunking (Modules 3-4)
pub mod chunk_id;

pub mod sentence;

pub mod chunker;

pub mod embedding_ops;

pub mod relevance_scorer;

pub mod threshold_config;

pub mod content_pruner;

// Re-exports for convenience (Modules 1-2)
pub use cache_config::{
    default_cache_dir, AiModel, CacheConfig, DEFAULT_MODEL_FILE, DEFAULT_MODEL_REPO,
    DEFAULT_MODEL_SHA256, MODEL_SELECTION_ENV,
};

pub use model_cache::ModelCache;

pub use model_downloader::{DownloadProgress, ModelDownloader};

pub use semantic_cleaner_impl::{ModelConfig, SemanticCleanerImpl};

pub use inference_engine::InferenceEngine;
pub use inference_engine::InferencePool;

pub use tokenizer::{MiniLmTokenizer, TokenBatch, DEFAULT_MAX_LENGTH};

pub use inference_engine::ModelInput;

// Re-exports for Semantic Chunking (Modules 3-4)
pub use chunk_id::ChunkId;

pub use sentence::SentenceSplitter;

pub use chunker::HtmlChunker;

pub use relevance_scorer::RelevanceScorer;

pub use threshold_config::ThresholdConfig;

pub use content_pruner::{ContentPruner, LegibleContentPruner, PruneAggressiveness};
