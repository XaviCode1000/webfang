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
//! This module is split by ONNX dependency, not by convenience:
//!
//! - **Text half** (no ONNX): `chunk_id`, `sentence`, `chunker`, `markdown_chunker`,
//!   `embedding_ops`, `relevance_scorer`, `threshold_config`, `content_pruner`, plus
//!   `cache_config` (pure model metadata: repo ids, file names, hashes, the `AiModel`
//!   enum — no `ort`/`tokenizers` edge). These compile without the `ai` feature, so
//!   the `EmbeddingPort` seam can be exercised with no local model.
//! - **The cleaner itself** (ADR-0004, slice C): `semantic_cleaner_impl` is now
//!   UNGATED too. It holds an erased
//!   [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort) and
//!   nothing engine-shaped, so
//!   a semantic cleaner over a REMOTE endpoint needs no ONNX build. Only its `new()`
//!   constructor — the local-ONNX convenience path that resolves a model off the hub
//!   — stays behind the feature.
//! - **ONNX half** (feature-gated behind `ai`): `inference_engine`, `tokenizer`,
//!   `embedding_adapter` (which now owns model resolution), `granite_dom_inspector`,
//!   the env compat layer and the test fixture.
//!
//! ```toml
//! [dependencies]
//! webfang = { version = "1.0", features = ["ai"] }
//! ```
//!
//! # Model Information
//!
//! - **Model**: IBM Granite embeddings (`ibm-granite/granite-embedding-97m-multilingual-r2`
//!   by default; Granite-311M (`granite-311m`) tier via `AI_MODEL_ID`)
//! - **Format**: ONNX (optimized for inference)
//! - **Size**: ~120MB (Granite-97M) / ~350MB (Granite-311M)
//! - **Max Tokens**: sequences truncate at 32,768 tokens (`DEFAULT_MAX_LENGTH`);
//!   chunk rejection is governed separately by `max_tokens`
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
//! # #[cfg(feature = "ai")]
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

// ONNX half (Modules 1-2) — gated on `ai`.

/// Model selection metadata (Granite-97M / Granite-311M repositories and
/// hashes).
///
/// UNGATED despite living in the ONNX half: it is a pure data module — repo
/// ids, file names, expected SHA256s, and the `AiModel` enum — with no `ort`
/// or `tokenizers` import at all. `ModelConfig` is ungated and holds an
/// `AiModel`, so the enum has to exist in every build. Ungating the
/// declaration adds nothing to the dependency graph; the gate it used to
/// carry was inherited from its module's old neighborhood, not earned by an
/// edge.
pub mod cache_config;

/// Backward-compat layer for environment variable naming (WEBFANG_AI_MODEL_ID / AI_MODEL_ID).
///
/// UNGATED, same reasoning as [`cache_config`]: it is pure std env-var
/// plumbing with no imports at all, and [`AiModel::from_env`]
/// (cache_config::AiModel::from_env) calls into it — so it has to exist
/// wherever `AiModel` does. Gating it forced the whole `cache_config` module
/// behind `ai`, which in turn forced the ungated cleaner's `ModelConfig`
/// behind it. Ungating the declaration adds no edge to the graph.
pub mod compat;

/// The semantic cleaner: prune → chunk → guard → embed through
/// [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort) →
/// Z-score relevance filter.
///
/// UNGATED (ADR-0004, slice C): it owns an erased embedding PORT and no
/// tokenizer, no engine, no model resolution. Everything ONNX-shaped it used
/// to do — `SemanticCleanerImpl::new`
/// and the hf_hub resolver behind it — moved to the `ai`-gated half, so this
/// module compiles with no `ai` feature and against ANY backend, remote
/// included.
pub mod semantic_cleaner_impl;

/// Adapter bridging `InferencePool` to the domain `EmbeddingPort` (#433).
#[cfg(feature = "ai")]
pub mod embedding_adapter;

#[cfg(feature = "ai")]
pub mod inference_engine;

#[cfg(feature = "ai")]
pub mod tokenizer;

// Text half — no ONNX dependency, available without the `ai` feature.

/// Unique identifier for content chunks with newtype safety.
pub mod chunk_id;

pub mod sentence;

pub mod chunker;

pub mod markdown_chunker;

pub mod embedding_ops;

pub mod relevance_scorer;

pub mod threshold_config;

pub mod content_pruner;

// Tier 2 DOM inspector — ONNX half: it imports `tokenizer` and
// `inference_engine` (`granite_dom_inspector.rs:21-22`), so it cannot compile
// without them even though its relevance scoring is plain cosine similarity.
#[cfg(feature = "ai")]
pub mod granite_dom_inspector;

/// Shared AI test fixture (#1575) — the in-memory WordPiece tokenizer used by
/// this crate's unit tests AND, via `#[path = "ai_test_fixture.rs"]`, by
/// `tests/erased_engine_ports_test.rs`.
///
/// `#[cfg(all(test, feature = "ai"))]` is load-bearing, not decoration: an
/// integration test links a library compiled WITHOUT `cfg(test)`, so this
/// declaration gives the library's own unit tests the fixture while the
/// integration test supplies its own `#[path]` include. Neither path reaches a
/// production build. The `feature = "ai"` arm keeps the fixture out of a
/// `cargo test` build without ONNX, where nothing would use it. See the
/// module's own docs for why no dev-dependency crate hosts it instead.
#[cfg(all(test, feature = "ai"))]
mod ai_test_fixture;

// Re-exports for convenience (Modules 1-2)

pub use cache_config::{AiModel, DEFAULT_MODEL_FILE, DEFAULT_MODEL_REPO, DEFAULT_MODEL_SHA256};

pub use semantic_cleaner_impl::{ModelConfig, SemanticCleanerImpl};

#[cfg(feature = "ai")]
pub use embedding_adapter::EmbeddingAdapter;

#[cfg(feature = "ai")]
pub use inference_engine::{
    build_engine, EngineConfig, InferenceEngine, InferencePool, MockInferenceEngine,
    PooledInferenceEngine, SingleSessionEngine,
};

#[cfg(feature = "ai")]
pub use tokenizer::{MiniLmTokenizer, TokenBatch, DEFAULT_MAX_LENGTH};

#[cfg(feature = "ai")]
pub use inference_engine::ModelInput;

// Re-exports for Semantic Chunking (Modules 3-4)
pub use chunk_id::ChunkId;

pub use sentence::SentenceSplitter;

pub use chunker::HtmlChunker;

pub use markdown_chunker::MarkdownChunker;

pub use relevance_scorer::RelevanceScorer;

pub use threshold_config::ThresholdConfig;

pub use content_pruner::{ContentPruner, LegibleContentPruner, PruneAggressiveness};

#[cfg(feature = "ai")]
pub use granite_dom_inspector::GraniteDomInspector;
