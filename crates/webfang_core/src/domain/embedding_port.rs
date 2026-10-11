//! Embedding port — async domain trait for text vectorization.
//!
//! Defines the contract for generating embedding vectors from text.
//! Concrete implementations live in the `webfang_ai` crate (wrapping
//! `InferencePool`). The trait uses `BoxFuture` for dyn-compatibility,
//! matching the pattern established in [`super::semantic_inspector`] and
//! [`super::repository::VectorRepository`].
//!
//! # Design decisions
//!
//! - **Not sealed**: unlike `SemanticCleaner`, this trait is open for
//!   testing (mock embeddings) and future alternative backends.
//! - **Always compiled**: the trait definition has no `#[cfg(feature = "ai")]`
//!   guard. The Container stores `Option<Arc<dyn EmbeddingPort>>` which is
//!   `None` when the `ai` feature is disabled.
//! - **Separate from `SemanticCleaner`**: the cleaner is a sealed, HTML-only
//!   pipeline. This port exposes the raw embedding primitive needed by vault
//!   search (issue #386) and any future semantic feature.
//! - **Origin is part of the type, not of the value**: the trait object alone
//!   erases *where* a port points. [`LocalEmbeddingPort`] carries that origin
//!   so a remote provider can never be substituted where only the local stack
//!   is legitimate (#1977), and [`cosine_similarity_fail_closed`] carries the
//!   ranking contract both `application` and `infrastructure` read (#1125).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::error::SemanticError;

/// A boxed future for dyn-compatible async traits.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Domain trait for text embedding generation.
///
/// Implementations wrap an ONNX inference pool to produce fixed-dimension
/// embedding vectors. The default model (Granite-97M) produces 384-dimensional
/// vectors; both supported models output 384d via Matryoshka truncation.
///
/// # Errors
///
/// Returns [`SemanticError`] when:
/// - [`SemanticError::Inference`]: ONNX model execution failed
/// - [`SemanticError::Tokenize`]: input text tokenization failed
/// - [`SemanticError::ModelLoad`]: model not available
pub trait EmbeddingPort: Send + Sync {
    /// Generate an embedding vector for a single text.
    ///
    /// Returns a fixed-dimension vector (384d for both Granite models).
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError::Inference`] if the model execution fails,
    /// or [`SemanticError::Tokenize`] if the text cannot be tokenized.
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>>;

    /// Generate embedding vectors for multiple texts in a batch.
    ///
    /// The default implementation calls [`embed`](Self::embed) sequentially.
    /// Implementations may override this to use batched ONNX inference for
    /// better throughput.
    ///
    /// Returns one vector per input text, in the same order.
    ///
    /// # Errors
    ///
    /// Returns [`SemanticError`] if any individual embedding fails.
    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
        Box::pin(async move {
            let mut results = Vec::with_capacity(texts.len());
            for text in texts {
                results.push(self.embed(text).await?);
            }
            Ok(results)
        })
    }

    /// The dimensionality of embedding vectors produced by this port.
    ///
    /// Both supported models (Granite-97M and Granite-311M) return 384.
    fn embedding_dim(&self) -> usize;
}

/// An [`EmbeddingPort`] whose origin is known to be the LOCAL embedding stack.
///
/// # Why the newtype exists
///
/// `VaultAiPorts::embedding_port` is `Option<Arc<dyn EmbeddingPort>>`, and
/// `resolve_remote_embedding` overwrites that slot with a remote adapter the
/// user opted into for **vault search only**. The slot is a `OnceCell` that
/// records no origin, so after the overwrite nothing in the type system
/// distinguishes "local ONNX" from "third-party endpoint".
///
/// That matters because Tier 2 semantic repair feeds DOM fragments to whatever
/// port it holds. Both sides have the SAME type, so the slot is a valid
/// argument for the inspector: `semantic_inspector(&vault_ports.embedding_port, …)`
/// compiles with exit 0 and ships page content off-box without consent
/// (issue #1977, ADR-0004 §Egress).
///
/// Wrapping the local port gives the two origins different types, which turns
/// that mistake into a type error instead of a code-review promise.
///
/// # Why the constructor is `pub` and there is no sealer
///
/// Rust visibility is per *crate*: a `pub(crate)` marker inside `webfang_core`
/// would not constrain `webfang_cli` or `webfang_ai`, which are exactly the
/// crates that must not mint one. A `pub` constructor is therefore the correct
/// surface, and the constraint that matters is review of its call sites —
/// exactly one **production** call site, the CLI composition root
/// (`build_ai_cleaner`), which wraps the port the ONNX stack handed back;
/// tests mint their own. This boundary is a convention backed by the compiler
/// where it can be (the newtype), and a reviewed convention where it cannot.
#[derive(Clone)]
pub struct LocalEmbeddingPort(Arc<dyn EmbeddingPort>);

impl LocalEmbeddingPort {
    /// Wrap a port already known to be the local ONNX stack.
    #[must_use]
    pub fn new(port: Arc<dyn EmbeddingPort>) -> Self {
        Self(port)
    }

    /// Borrow the wrapped port.
    #[must_use]
    pub fn port(&self) -> &Arc<dyn EmbeddingPort> {
        &self.0
    }
}

/// Cosine similarity that **fails closed** on a dimension mismatch.
///
/// Returns `Some(score)` in `[-1.0, 1.0]`, and `Some(0.0)` for empty or
/// zero-magnitude vectors — a real absence of signal, not a broken contract.
/// Returns `None` when the two vectors have different lengths, and the caller
/// **must** skip that vector.
///
/// # Why this lives in `domain`
///
/// Both consumers sit on the inward side of the port — `application::vault_search`
/// (#1125) and `infrastructure::scraper::embedding_semantic_inspector`
/// (ADR-0004) — and importing a private `fn` from `application/` into
/// `infrastructure/` is an outward reference the intra-crate gate forbids
/// (`target_rank < src_rank`). `domain` is the one layer both may read, and
/// keeping one copy here is what stops the two from drifting apart.
///
/// # Why it must never truncate
///
/// Two other cosines in this workspace compare over `a.len().min(b.len())`:
/// `webfang_ai::infrastructure_ai::embedding_ops::cosine_similarity` and
/// `webfang_core::infrastructure::export::vector_exporter::cosine_similarity`.
/// Truncation turns a length mismatch into a *plausible* score computed over a
/// partial vector: `[1.0, 0.0]` against `[1.0, 0.0, 0.0]` reports 1.0 — the
/// maximum — so a corrupt vector can WIN a ranking instead of losing it. That
/// is precisely the failure `None` makes unrepresentable.
pub fn cosine_similarity_fail_closed(a: &[f32], b: &[f32]) -> Option<f32> {
    if a.len() != b.len() {
        return None;
    }
    if a.is_empty() {
        return Some(0.0);
    }

    let mut dot = 0.0f32;
    let mut mag_a = 0.0f32;
    let mut mag_b = 0.0f32;

    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        mag_a += x * x;
        mag_b += y * y;
    }

    let denom = mag_a.sqrt() * mag_b.sqrt();
    if denom < f32::EPSILON {
        Some(0.0)
    } else {
        Some(dot / denom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic mock that returns unit vectors based on text length.
    struct MockEmbedder {
        dim: usize,
    }

    impl EmbeddingPort for MockEmbedder {
        fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
            Box::pin(async move {
                let mut v = vec![0.0f32; self.dim];
                // Deterministic: use text length mod dim as the hot index.
                let idx = text.len() % self.dim;
                v[idx] = 1.0;
                Ok(v)
            })
        }

        fn embedding_dim(&self) -> usize {
            self.dim
        }
    }

    #[tokio::test]
    async fn test_mock_embedder_produces_correct_dim() {
        let embedder = MockEmbedder { dim: 384 };
        let v = embedder.embed("hello world").await.unwrap();
        assert_eq!(v.len(), 384);
        assert_eq!(embedder.embedding_dim(), 384);
    }

    #[tokio::test]
    async fn test_mock_embedder_deterministic() {
        let embedder = MockEmbedder { dim: 384 };
        let v1 = embedder.embed("test").await.unwrap();
        let v2 = embedder.embed("test").await.unwrap();
        assert_eq!(v1, v2, "same input must produce same embedding");
    }

    #[tokio::test]
    async fn test_embed_batch_default_impl() {
        let embedder = MockEmbedder { dim: 4 };
        let texts = vec!["a".to_string(), "bb".to_string(), "ccc".to_string()];
        let results = embedder.embed_batch(&texts).await.unwrap();
        assert_eq!(results.len(), 3);
        for (i, v) in results.iter().enumerate() {
            assert_eq!(v.len(), 4, "batch item {i} must have correct dim");
        }
    }

    #[tokio::test]
    async fn test_embed_batch_empty_input() {
        let embedder = MockEmbedder { dim: 384 };
        let texts: Vec<String> = vec![];
        let results = embedder.embed_batch(&texts).await.unwrap();
        assert!(results.is_empty(), "empty input must produce empty output");
    }

    #[test]
    fn test_embedding_port_is_object_safe() {
        fn assert_dyn_compatible(_: &dyn EmbeddingPort) {}
        let embedder = MockEmbedder { dim: 384 };
        assert_dyn_compatible(&embedder);
    }
}
