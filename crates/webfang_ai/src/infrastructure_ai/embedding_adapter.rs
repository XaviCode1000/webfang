//! Embedding adapter — bridges an erased [`InferenceEngine`] to the domain [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort).
//!
//! This is the concrete infrastructure implementation of the always-compiled
//! domain port [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort). It wraps an erased ONNX [`InferenceEngine`] (`Arc<dyn
//! InferenceEngine + Send + Sync>` — the single-session [`InferencePool`] or
//! the N-session
//! [`PooledInferenceEngine`](crate::infrastructure_ai::inference_engine::PooledInferenceEngine)
//! alike, #1569) and the
//! HuggingFace [`MiniLmTokenizer`] to turn raw text into fixed-dimension
//! embedding vectors for IBM Granite embedding models (Granite-97M / Granite-311M,
//! unified 384d output), following the Adapter pattern (infrastructure adapts the
//! domain port to the ONNX primitives). Note: `MiniLmTokenizer` is legacy naming
//! from the pre-Granite era; an API rename is tracked as a follow-up refactor.
//!
//! # Architecture
//!
//! ```text
//! &str / &[String]
//!     ↓
//! [MiniLmTokenizer] text → ModelInput (token ids + masks)   // legacy name; see note above
//!     ↓
//! erased [InferenceEngine] ModelInput → Vec<f32>
//!     ↓
//! Vec<f32> / Vec<Vec<f32>>
//! ```
//!
//! # Design decisions
//!
//! - **Batch override**: [`EmbeddingPort::embed_batch`](webfang_core::domain::embedding_port::EmbeddingPort::embed_batch) defaults to calling
//!   [`EmbeddingPort::embed`](webfang_core::domain::embedding_port::EmbeddingPort::embed) per text, re-tokenizing through the port surface
//!   each time. This adapter overrides it to tokenize the whole batch in one
//!   [`MiniLmTokenizer::tokenize_batch`] call, then dispatches the resulting
//!   [`ModelInput`]s to the pool. `InferencePool` has no true batched inference
//!   (each `infer` is one worker request), so the dispatch is sequential — the
//!   win is avoiding per-call re-tokenization overhead, not parallel ONNX.
//! - **Span attachment**: the port methods are synchronous fns returning a
//!   `BoxFuture`, so `#[instrument]` would only span the (trivial) future
//!   construction. Per the observability contract, the span is attached to the
//!   future itself via [`Instrument::instrument`](tracing::Instrument::instrument) so it covers the actual
//!   tokenize + infer work.
//! - **Erased engine (#1569)**: the adapter stores
//!   `Arc<dyn InferenceEngine + Send + Sync>` instead of the concrete
//!   [`InferencePool`], so a Pool-mode cleaner shares its N-session engine with
//!   vault search through one model load — the `Single` path compiles unchanged
//!   via unsized coercion.
//! - **Shared resolution**: [`EmbeddingAdapter::from_config`] reuses
//!   `resolve_model_assets`,
//!   the same hf_hub cache/download + SHA256 validation path extracted from
//!   `SemanticCleanerImpl::new`, so both pipelines resolve models identically.
//!
//! # Rust-Skills Applied
//!
//! - `own-arc-shared`: erased `Arc<dyn InferenceEngine + Send + Sync>` /
//!   `Arc<MiniLmTokenizer>` shared ownership
//! - `async-clone-before-await`: references captured before await points
//! - `err-question-mark`: `?` propagation, no `.unwrap()` in production
//! - `obs-instrument-spans`: spans attached to the boxed futures
//! - `mem-with-capacity`: batch result pre-allocation

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::{debug, Instrument};

use crate::infrastructure_ai::inference_engine::{InferenceEngine, InferencePool};
use crate::infrastructure_ai::semantic_cleaner_impl::{resolve_model_assets, ModelConfig};
use crate::infrastructure_ai::tokenizer::MiniLmTokenizer;
use crate::infrastructure_ai::{build_engine, EngineConfig};
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::error::SemanticError;

/// A boxed future for dyn-compatible async trait methods.
///
/// Mirrors the private alias in [`webfang_core::domain::embedding_port`]; the
/// alias there is module-private, so the adapter declares its own.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The local embedding stack, built once: the erased domain port the cleaner
/// consumes, plus the erased engine and tokenizer the vault-search port and
/// the Tier 2 inspector still need.
pub type OnnxEmbeddingStack = (
    Arc<dyn EmbeddingPort>,
    Arc<dyn InferenceEngine + Send + Sync>,
    Arc<MiniLmTokenizer>,
);

/// Build the local ONNX embedding port AND the engine + tokenizer behind it.
///
/// This is the one composition root for the local embedding stack (ADR-0004):
/// it resolves and validates the model assets once, builds the selected
/// engine, loads the tokenizer, and wraps all three in an
/// [`EmbeddingAdapter`] erased to the domain [`EmbeddingPort`].
///
/// The three-tuple exists so every caller keeps what it needs without asking
/// the cleaner for it: the semantic cleaner takes only the port
/// ([`SemanticCleanerImpl::from_parts`](crate::infrastructure_ai::SemanticCleanerImpl::from_parts)),
/// while the vault-search embedding port and the Tier 2 DOM inspector still
/// want the erased engine and the tokenizer directly. Handing those out here
/// is what keeps the `--ai` path at ONE model load per process.
///
/// `EngineConfig::Single` behaves like the historical single-session pool
/// (graceful degradation included); `Pool { size }` opens `size` sessions.
///
/// # Errors
///
/// Returns [`SemanticError`] when model resolution or download fails, the
/// SHA256 integrity check fails, the tokenizer cannot be loaded, or the engine
/// cannot be built (a pool session that cannot be built fails fast instead of
/// degrading).
#[tracing::instrument(skip(config), fields(repo = %config.repo, offline_mode = config.offline_mode, engine = ?engine_config))]
pub async fn build_onnx_embedding_port(
    config: &ModelConfig,
    engine_config: EngineConfig,
) -> Result<OnnxEmbeddingStack, SemanticError> {
    let (model_path, tokenizer_path) = resolve_model_assets(config).await?;
    let tokenizer = Arc::new(MiniLmTokenizer::from_file(&tokenizer_path).await?);
    let engine = build_engine(&engine_config, model_path, config.model_variant)?;
    debug!(
        dim = engine.embedding_dim(),
        "ONNX embedding port built (engine, tokenizer and port share one model)"
    );
    let port: Arc<dyn EmbeddingPort> = Arc::new(EmbeddingAdapter::new(
        Arc::clone(&engine),
        Arc::clone(&tokenizer),
    ));
    Ok((port, engine, tokenizer))
}

/// Adapter exposing an erased [`InferenceEngine`] through the domain [`EmbeddingPort`].
///
/// Cheap to clone-share via `Arc`: both fields are `Arc`-wrapped. The engine is
/// type-erased (`Arc<dyn InferenceEngine + Send + Sync>`, #1569) so the SAME
/// engine instance backs the semantic cleaner, vault-search embeddings and the
/// Tier 2 inspector regardless of whether it is the single-session
/// [`InferencePool`] or the N-session
/// [`PooledInferenceEngine`](crate::infrastructure_ai::inference_engine::PooledInferenceEngine).
/// Concrete
/// `Arc<InferencePool>` arguments coerce into the erased field at the
/// [`EmbeddingAdapter::new`] call site, so the Single-path constructors keep
/// compiling unchanged. Constructed either directly from existing components
/// ([`EmbeddingAdapter::new`]) or from a [`ModelConfig`] that resolves and
/// validates the model + tokenizer ([`EmbeddingAdapter::from_config`]).
///
/// # Thread Safety
///
/// `Send + Sync` — erased `dyn InferenceEngine + Send + Sync` and
/// `MiniLmTokenizer` are `Send + Sync`, so the adapter can be shared as
/// `Arc<dyn EmbeddingPort>` across the MCP server and any future consumer.
pub struct EmbeddingAdapter {
    /// Erased ONNX inference engine (single-session pool or N-session pool).
    pool: Arc<dyn InferenceEngine + Send + Sync>,
    /// HuggingFace tokenizer (text → `ModelInput`).
    tokenizer: Arc<MiniLmTokenizer>,
}

impl EmbeddingAdapter {
    /// Wrap an existing inference engine and tokenizer.
    ///
    /// The engine is accepted erased; a concrete `Arc<InferencePool>` (the
    /// Single path) coerces implicitly. Use this when the caller already holds
    /// the components; otherwise prefer
    /// [`build_onnx_embedding_port`] (which also resolves the model) or
    /// [`from_config`](Self::from_config).
    #[must_use]
    pub fn new(
        pool: Arc<dyn InferenceEngine + Send + Sync>,
        tokenizer: Arc<MiniLmTokenizer>,
    ) -> Self {
        Self { pool, tokenizer }
    }

    /// Resolve the model + tokenizer from `config` and build the adapter.
    ///
    /// Mirrors `SemanticCleanerImpl::new`: resolves the model and tokenizer
    /// through the hf_hub cache (cache-first online, strict offline), validates
    /// the model SHA256 by streaming the file on disk, then loads the
    /// tokenizer and builds the inference pool.
    ///
    /// Callers: the semantic cleaner's own
    /// [`SemanticCleanerImpl::new`](crate::infrastructure_ai::SemanticCleanerImpl::new)
    /// constructor (the Single path), plus the offline-failure test below. The
    /// `Pool`-capable composition root is [`build_onnx_embedding_port`].
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError`] when model resolution, download, SHA256
    /// validation, tokenizer loading, or pool construction fails.
    #[tracing::instrument(skip(config), fields(repo = %config.repo, offline_mode = config.offline_mode))]
    pub async fn from_config(config: &ModelConfig) -> Result<Self, SemanticError> {
        let (model_path, tokenizer_path) = resolve_model_assets(config).await?;
        let tokenizer = Arc::new(MiniLmTokenizer::from_file(&tokenizer_path).await?);
        let pool: Arc<dyn InferenceEngine + Send + Sync> =
            Arc::new(InferencePool::new(model_path, config.model_variant)?);
        debug!(dim = pool.embedding_dim(), "EmbeddingAdapter initialized");
        Ok(Self { pool, tokenizer })
    }
}

impl EmbeddingPort for EmbeddingAdapter {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        let span = tracing::debug_span!("embed", text_len = text.len(), dim = self.embedding_dim());
        Box::pin(
            async move {
                let input = self.tokenizer.tokenize(text)?;
                self.pool.infer(&input).await
            }
            .instrument(span),
        )
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
        let span = tracing::debug_span!(
            "embed_batch",
            count = texts.len(),
            dim = self.embedding_dim()
        );
        Box::pin(
            async move {
                if texts.is_empty() {
                    return Ok(Vec::new());
                }
                // Tokenize the whole batch once (avoids per-call re-tokenization),
                // then dispatch each ModelInput to the pool sequentially — the pool
                // has no batched inference, each infer is a single worker request.
                let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
                let batch = self.tokenizer.tokenize_batch(&refs)?;
                let inputs = batch.to_model_inputs();
                let mut results = Vec::with_capacity(inputs.len());
                for input in &inputs {
                    results.push(self.pool.infer(input).await?);
                }
                Ok(results)
            }
            .instrument(span),
        )
    }

    fn embedding_dim(&self) -> usize {
        self.pool.embedding_dim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure_ai::ai_test_fixture::in_memory_wordpiece_tokenizer;
    use crate::infrastructure_ai::inference_engine::MockInferenceEngine;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    /// Adapter over a fixed-latency mock engine and an in-memory tokenizer — no
    /// ONNX model, no worker threads, fully deterministic.
    ///
    /// The mock reports the same 384-dim output as `AiModel::Granite97M`, which
    /// is all these tests assert. #1575 switched this fixture off the
    /// unloadable-model `InferencePool` it used to build: the pool spawns a
    /// worker thread per instance purely so a test can read a dimension, and
    /// that setup was a byte-for-byte copy of the one in
    /// `tests/erased_engine_ports_test.rs`. The pool's own configured dimension
    /// stays covered by the `inference_engine` unit tests, and the concrete-pool
    /// path through this adapter stays covered by the integration suite, which
    /// needs the concrete type to prove unsized coercion.
    fn fake_adapter() -> EmbeddingAdapter {
        let engine: Arc<dyn InferenceEngine + Send + Sync> = Arc::new(MockInferenceEngine::new(
            std::time::Duration::from_millis(1),
        ));
        let tokenizer = Arc::new(MiniLmTokenizer::new(in_memory_wordpiece_tokenizer(), 512));
        EmbeddingAdapter::new(engine, tokenizer)
    }

    #[test]
    fn test_embedding_adapter_is_send_sync() {
        assert_send::<EmbeddingAdapter>();
        assert_sync::<EmbeddingAdapter>();
    }

    #[test]
    fn test_embedding_dim_returns_384() {
        let adapter = fake_adapter();
        assert_eq!(
            adapter.embedding_dim(),
            384,
            "the engine's Granite-97M output dim must report 384d"
        );
    }

    #[test]
    fn test_adapter_coerces_to_dyn_embedding_port() {
        // Object-safety proof with a real instance: the adapter must coerce to
        // the erased domain port and delegate embedding_dim through the vtable.
        let adapter = fake_adapter();
        let port: &dyn EmbeddingPort = &adapter;
        assert_eq!(port.embedding_dim(), 384);
    }

    #[tokio::test]
    async fn test_from_config_fails_offline_without_cache() {
        // Offline mode + a bogus repo id (never present in the hf_hub cache)
        // guarantees a deterministic resolution failure without network access,
        // mirroring the SemanticCleanerImpl::new offline tests.
        let config = ModelConfig::new()
            .with_repo("nonexistent/fake-repo-for-test")
            .with_offline_mode(true);
        let result = EmbeddingAdapter::from_config(&config).await;
        assert!(
            result.is_err(),
            "offline resolution of an uncached model must fail"
        );
    }
}
