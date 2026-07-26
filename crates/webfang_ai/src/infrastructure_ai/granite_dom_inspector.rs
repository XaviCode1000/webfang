//! Granite-97M based semantic inspector for Tier 2 selector repair.
//!
//! Implements [`SemanticInspectorPort`] using the existing [`InferencePool`]
//! for ONNX inference and [`MiniLmTokenizer`] for tokenization. Embeddings
//! are compared via cosine similarity to find the best matching CSS selector.

use std::sync::Arc;

use tracing::debug;

use webfang_core::domain::dom_inspector::SelectorErrorKind;
use webfang_core::domain::semantic_inspector::{
    BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};

use super::embedding_ops::cosine_similarity;
use super::inference_engine::InferencePool;
use super::tokenizer::MiniLmTokenizer;

/// Semantic inspector powered by Granite-97M embeddings via ONNX inference.
pub struct GraniteDomInspector {
    pool: Arc<InferencePool>,
    tokenizer: Arc<MiniLmTokenizer>,
    threshold: f32,
}

impl GraniteDomInspector {
    /// Create a new inspector with the given inference pool and tokenizer.
    #[must_use]
    pub fn new(pool: Arc<InferencePool>, tokenizer: Arc<MiniLmTokenizer>, threshold: f32) -> Self {
        Self {
            pool,
            tokenizer,
            threshold,
        }
    }
}

impl SemanticInspectorPort for GraniteDomInspector {
    fn find_semantic_match<'a>(
        &'a self,
        ctx: SemanticContext,
    ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>> {
        Box::pin(async move {
            if ctx.dom_fragments.is_empty() {
                debug!("no dom fragments to compare against");
                return Ok(None);
            }

            // Tokenize the target text
            let target_input = self.tokenizer.tokenize(&ctx.target_text).map_err(|e| {
                SelectorErrorKind::InvalidSelector(format!("tokenization failed: {e}"))
            })?;

            // Get embedding for target text
            let target_embedding = self.pool.infer(&target_input).await.map_err(|e| {
                SelectorErrorKind::InvalidSelector(format!("inference failed: {e}"))
            })?;

            // Compare against each DOM fragment
            let mut best_score = 0.0f32;
            let mut best_selector = String::new();

            for fragment in &ctx.dom_fragments {
                let fragment_input = self.tokenizer.tokenize(fragment).map_err(|e| {
                    SelectorErrorKind::InvalidSelector(format!("tokenization failed: {e}"))
                })?;

                let fragment_embedding = self.pool.infer(&fragment_input).await.map_err(|e| {
                    SelectorErrorKind::InvalidSelector(format!("inference failed: {e}"))
                })?;

                let score = cosine_similarity(&target_embedding, &fragment_embedding);
                if score > best_score {
                    best_score = score;
                    best_selector = fragment.clone();
                }
            }

            debug!(
                best_score,
                best_selector = %best_selector,
                threshold = self.threshold,
                "semantic match evaluated"
            );

            if best_score >= self.threshold {
                Ok(Some(SemanticMatch {
                    selector: best_selector,
                    confidence: best_score,
                    source: TierSource::Semantic,
                }))
            } else {
                Ok(None)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    // Note: GraniteDomInspector requires real InferencePool and MiniLmTokenizer.
    // Unit tests use MockSemanticInspector in adaptive_engine tests instead.
    // Integration tests should use real models with #[ignore] annotation.
}
