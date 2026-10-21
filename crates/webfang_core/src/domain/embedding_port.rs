//! Embedding port — async domain trait for text vectorization.
//!
//! Defines the contract for generating embedding vectors from text.
//! The concrete implementation lives in `infrastructure::semantic_cleaner`
//! (ADR-0004 paso 6 removed the local ONNX stack). The trait uses `BoxFuture`
//! for dyn-compatibility, matching the pattern established in
//! [`super::semantic_inspector`] and [`super::repository::VectorRepository`].
//!
//! # Design decisions
//!
//! - **Not sealed**: unlike `SemanticCleaner`, this trait is open for
//!   testing (mock embeddings) and future alternative backends.
//! - **Always compiled**: the trait definition carries no `cfg` guard. The
//!   Container stores `Option<Arc<dyn EmbeddingPort>>` which is `None` when
//!   no embedding backend was wired.
//! - **Separate from `SemanticCleaner`**: the cleaner is a sealed, HTML-only
//!   pipeline. This port exposes the raw embedding primitive needed by vault
//!   search (issue #386) and any future semantic feature.
//! - **The slot is part of the type, not of the value**: the trait object
//!   alone erases *which slot* a port occupies.
//!   [`Tier2EmbeddingPort`] carries the slot so the vault-search port can
//!   never be substituted where only the Tier 2 slot is legitimate (F1 in
//!   `odd/tasks/tier2-port-backed-inspector.md`, restored here); and
//!   [`cosine_similarity_fail_closed`] carries the ranking contract both
//!   `application` and `infrastructure` read (#1125).

use std::any::Any;
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
/// - [`SemanticError::Inference`]: the embedding pipeline failed — the remote
///   endpoint refused or returned something unusable, or the vectors did not
///   line up with the chunks
/// - [`SemanticError::Tokenize`]: input text could not be chunked
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

    /// Returns a reference to the concrete implementation as `&dyn Any`,
    /// enabling downcasting for implementation-specific metrics.
    ///
    /// Default implementation panics; implementations SHOULD override this
    /// to return `self` as `&dyn Any`. This is a controlled escape hatch for
    /// telemetry collection (e.g., embedding usage counters) without leaking
    /// implementation details into the domain trait.
    fn as_any(&self) -> &dyn Any {
        panic!("as_any not implemented for EmbeddingPort; concrete type must override");
    }
}

/// The Tier 2 semantic embedding slot — *which slot* a port occupies.
///
/// # Why this exists
///
/// `VaultAiPorts::embedding_port` is `Option<Arc<dyn EmbeddingPort>>` — the
/// same erased type a Tier 2 port would carry, so `semantic_inspector`
/// accepted the vault slot and it compiled with exit 0, feeding DOM fragments
/// to whatever the vault slot had been overwritten with (issue #1977, ADR-0004
/// §Egress). A distinct type is what keeps the two slots
/// non-interchangeable: the vault slot is not assignable to
/// `Option<Tier2EmbeddingPort>`.
///
/// Deliberately does NOT implement [`EmbeddingPort`]: [`port`](Self::port) is
/// the only way out, so the erased form cannot be reached by an implicit
/// coercion.
///
/// # What it does NOT carry
///
/// The "origin" axis (local stack vs remote provider) is gone: ADR-0004 paso 6
/// removed the local ONNX stack, so the only producer today is a remote
/// provider the operator consented to — consent is the caller's contract
/// (`build_tier2_embedding_provider` in `crate::cli::llm_wire` refuses before
/// the dial). This type marks the SLOT, not the origin: it says nothing about
/// where the port points, and it does not prove that consent was given.
#[derive(Clone)]
pub struct Tier2EmbeddingPort(Arc<dyn EmbeddingPort>);

impl Tier2EmbeddingPort {
    /// Wrap a port resolved for the Tier 2 semantic slot.
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

        fn as_any(&self) -> &dyn Any {
            self
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

    /// The newtype hands back the SAME port it was built from, so a caller
    /// never has to unwrap it to embed: it must forward to the allocation the
    /// newtype was built from, not to a clone with a different refcount.
    #[test]
    fn tier2_port_borrows_the_underlying_port() {
        let erased: Arc<dyn EmbeddingPort> = Arc::new(MockEmbedder { dim: 384 });

        let slot = Tier2EmbeddingPort::new(Arc::clone(&erased));

        assert!(
            Arc::ptr_eq(slot.port(), &erased),
            "the newtype must forward, not copy"
        );
    }
}
