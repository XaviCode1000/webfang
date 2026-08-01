//! Embedding port — async domain trait for text vectorization.
//!
//! Defines the contract for generating embedding vectors from text.
//! Concrete implementations live in the `webfang_ai` crate (wrapping
//! [`InferencePool`]). The trait uses `BoxFuture` for dyn-compatibility,
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

use std::future::Future;
use std::pin::Pin;

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
