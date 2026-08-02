//! Embedding adapter — bridges [`InferencePool`] to the domain [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort).
//!
//! This is the concrete infrastructure implementation of the always-compiled
//! domain port [`EmbeddingPort`](webfang_core::domain::embedding_port::EmbeddingPort). It wraps the ONNX [`InferencePool`] and the
//! HuggingFace [`MiniLmTokenizer`] to turn raw text into fixed-dimension
//! embedding vectors, following the Adapter pattern (infrastructure adapts the
//! domain port to the ONNX primitives).
//!
//! # Architecture
//!
//! ```text
//! &str / &[String]
//!     ↓
//! [MiniLmTokenizer] text → ModelInput (token ids + masks)
//!     ↓
//! [InferencePool] ModelInput → Vec<f32> (dedicated worker threads)
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
//! - **Shared resolution**: [`EmbeddingAdapter::from_config`] reuses
//!   `resolve_model_assets`,
//!   the same hf_hub cache/download + SHA256 validation path extracted from
//!   `SemanticCleanerImpl::new`, so both pipelines resolve models identically.
//!
//! # Rust-Skills Applied
//!
//! - `own-arc-shared`: `Arc<InferencePool>` / `Arc<MiniLmTokenizer>` shared ownership
//! - `async-clone-before-await`: references captured before await points
//! - `err-question-mark`: `?` propagation, no `.unwrap()` in production
//! - `obs-instrument-spans`: spans attached to the boxed futures
//! - `mem-with-capacity`: batch result pre-allocation

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tracing::{debug, Instrument};

use crate::infrastructure_ai::inference_engine::InferencePool;
use crate::infrastructure_ai::semantic_cleaner_impl::{resolve_model_assets, ModelConfig};
use crate::infrastructure_ai::tokenizer::MiniLmTokenizer;
use webfang_core::domain::embedding_port::EmbeddingPort;
use webfang_core::error::SemanticError;

/// A boxed future for dyn-compatible async trait methods.
///
/// Mirrors the private alias in [`webfang_core::domain::embedding_port`]; the
/// alias there is module-private, so the adapter declares its own.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Adapter exposing [`InferencePool`] embeddings through the domain [`EmbeddingPort`].
///
/// Cheap to clone-share via `Arc`: both fields are `Arc`-wrapped. Constructed
/// either directly from existing components ([`EmbeddingAdapter::new`]) or from
/// a [`ModelConfig`] that resolves and validates the model + tokenizer
/// ([`EmbeddingAdapter::from_config`]).
///
/// # Thread Safety
///
/// `Send + Sync` — both `InferencePool` and `MiniLmTokenizer` are `Send + Sync`,
/// so the adapter can be shared as `Arc<dyn EmbeddingPort>` across the MCP server
/// and any future consumer.
pub struct EmbeddingAdapter {
    /// ONNX inference pool (dedicated worker threads with persistent sessions).
    pool: Arc<InferencePool>,
    /// HuggingFace tokenizer (text → `ModelInput`).
    tokenizer: Arc<MiniLmTokenizer>,
}

impl EmbeddingAdapter {
    /// Wrap an existing inference pool and tokenizer.
    ///
    /// Use this when the caller already holds the components (e.g. sharing the
    /// pool built for another pipeline); otherwise prefer [`from_config`](Self::from_config).
    #[must_use]
    pub fn new(pool: Arc<InferencePool>, tokenizer: Arc<MiniLmTokenizer>) -> Self {
        Self { pool, tokenizer }
    }

    /// Resolve the model + tokenizer from `config` and build the adapter.
    ///
    /// Mirrors `SemanticCleanerImpl::new`: resolves the model and tokenizer
    /// through the hf_hub cache (cache-first online, strict offline), validates
    /// the model SHA256 in memory, then loads the tokenizer and builds the
    /// inference pool.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError`] when model resolution, download, SHA256
    /// validation, tokenizer loading, or pool construction fails.
    #[tracing::instrument(skip(config), fields(repo = %config.repo, offline_mode = config.offline_mode))]
    pub async fn from_config(config: &ModelConfig) -> Result<Self, SemanticError> {
        let (model_bytes, tokenizer_path) = resolve_model_assets(config).await?;
        let tokenizer = Arc::new(MiniLmTokenizer::from_file(&tokenizer_path).await?);
        let pool = Arc::new(InferencePool::new(model_bytes, config.model_variant)?);
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
    use crate::infrastructure_ai::cache_config::AiModel;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    /// Build a minimal in-memory WordPiece tokenizer — no tokenizer.json file
    /// required. Only needs to EXIST for adapter construction; the embedding_dim
    /// tests never invoke tokenization.
    fn in_memory_tokenizer() -> tokenizers::Tokenizer {
        use tokenizers::models::wordpiece::WordPiece;
        // `WordPieceBuilder::vocab` accepts `Into<AHashMap>`; an array of tuples
        // converts directly (avoids a std HashMap → AHashMap mismatch).
        let vocab = [
            ("[PAD]".to_string(), 0u32),
            ("[UNK]".to_string(), 100),
            ("[CLS]".to_string(), 101),
            ("[SEP]".to_string(), 102),
            ("hello".to_string(), 5),
            ("world".to_string(), 6),
        ];
        let model = WordPiece::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .build()
            .expect("wordpiece model must build from an inline vocab");
        tokenizers::Tokenizer::new(model)
    }

    /// Adapter backed by fake model bytes (workers fail async but the pool still
    /// reports its configured dimension) and an in-memory tokenizer — no ONNX
    /// model download, fully deterministic.
    fn fake_adapter() -> EmbeddingAdapter {
        let pool = Arc::new(
            InferencePool::new(
                Arc::new(b"not a real onnx model".to_vec()),
                AiModel::Granite97M,
            )
            .expect("pool creation must succeed even with invalid model bytes"),
        );
        let tokenizer = Arc::new(MiniLmTokenizer::new(in_memory_tokenizer(), 512));
        EmbeddingAdapter::new(pool, tokenizer)
    }

    #[test]
    fn test_embedding_adapter_is_send_sync() {
        assert_send::<EmbeddingAdapter>();
        assert_sync::<EmbeddingAdapter>();
    }

    #[test]
    fn test_embedding_dim_returns_384() {
        let adapter = fake_adapter();
        assert_eq!(adapter.embedding_dim(), 384, "Granite-97M must report 384d");
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
