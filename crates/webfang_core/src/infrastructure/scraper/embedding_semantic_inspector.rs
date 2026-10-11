//! Embedding-backed Tier 2 semantic inspector — infrastructure layer.
//!
//! Implements [`SemanticInspectorPort`] over the erased [`EmbeddingPort`] seam
//! (ADR-0004). The target text and every DOM fragment go out in ONE
//! `embed_batch` call; the returned vectors are ranked locally. Symmetric with
//! [`DefaultDomInspector`](super::dom_inspector::DefaultDomInspector): Tier 1 is
//! lexical and CPU-bound, Tier 2 asks the embedding port.
//!
//! # Why this type exists
//!
//! Two properties are load-bearing, and both are regressions against the
//! `GraniteDomInspector` it replaces:
//!
//! 1. **No ONNX, and one round trip.** Nothing here knows which backend
//!    produces the vectors, so Tier 2 survives the removal of the local ONNX
//!    stack and works unchanged against a remote endpoint. The old inspector
//!    issued one `infer` per DOM fragment — an N+1 round trip over the
//!    inference pool, paid again on every failed selector.
//! 2. **Fail-closed cosine.** The comparison never truncates to the shorter
//!    vector, and never treats a mismatch as "similarity 0". A dimension
//!    mismatch means the port broke its contract (a mixed-model index, a
//!    profile whose width changed mid-run), and letting such a vector rank is
//!    how corrupt data wins a comparison. Mismatched vectors are dropped from
//!    the ranking — see the `cosine_similarity_checked` helper below.
//!
//! # Dimension-mismatch policy (degraded, not fatal)
//!
//! A mismatched fragment is SKIPPED, not turned into an error and not scored as
//! `0.0`. A mismatch is a property of one vector, and one bad vector in a batch
//! says nothing about its neighbours, so failing the whole call would discard
//! repair candidates that were perfectly comparable. When NO fragment remains
//! comparable the inspector degrades to `Ok(None)`: "Tier 2 found nothing",
//! which is the same signal a below-threshold result carries and which the
//! adaptive engine already handles by falling back to Tier 1
//! (`adaptive_engine/mod.rs`). The invariant this buys is the one that matters:
//! a mismatched vector can NEVER be reported as a match, and is never
//! indistinguishable from a genuine low score.
//!
//! # Batch cap
//!
//! [`ModelProfile::batch_max`](crate::domain::model_profile::ModelProfile::batch_max)
//! is honoured when the constructor is given one: a `[target, …fragments]`
//! batch longer than the cap is sent as consecutive windows of at most `cap`
//! texts and reassembled by position, so the index of every vector still
//! matches the index of its text. `None` (the default, and what every profile
//! declares today) means no cap and therefore exactly one round trip.
//! Declaring the cap is this type's job; deciding WHICH profile a run uses is
//! the composition edge's — no profile is wired to the CLI yet.

use std::sync::Arc;

use tracing::{debug, info_span, Instrument};

use crate::domain::dom_inspector::SelectorErrorKind;
use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::semantic_inspector::{
    BoxFuture, SemanticContext, SemanticInspectorPort, SemanticMatch, TierSource,
};
use crate::error::SemanticError;

/// Tier 2 semantic inspector over an erased embedding port.
///
/// The whole type is two fields: which port to ask, and the confidence bar a
/// candidate must clear. Everything backend-shaped — tokenizer, inference
/// engine, model resolution — belongs to the port implementation, which is why
/// this type compiles with no ONNX in the graph.
pub struct EmbeddingSemanticInspector {
    embedding: Arc<dyn EmbeddingPort>,
    threshold: f32,
    batch_max: Option<usize>,
}

impl EmbeddingSemanticInspector {
    /// Create an inspector over `embedding` that reports a match at or above
    /// `threshold`.
    ///
    /// No batch cap: `batch_max` stays `None` until
    /// [`with_batch_max`](Self::with_batch_max) says otherwise.
    #[must_use]
    pub fn new(embedding: Arc<dyn EmbeddingPort>, threshold: f32) -> Self {
        Self {
            embedding,
            threshold,
            batch_max: None,
        }
    }

    /// Cap how many texts go into a single `embed_batch` request.
    ///
    /// Mirrors
    /// [`ModelProfile::batch_max`](crate::domain::model_profile::ModelProfile::batch_max):
    /// `None` (and a cap of `0`, which is not a meaningful batch width) means
    /// no cap, so one request carries the whole `[target, …fragments]` batch. With a cap, the batch is split into
    /// consecutive windows and reassembled by position, so the returned vector
    /// at index `i` is still the embedding of the text at index `i`.
    #[must_use]
    pub fn with_batch_max(mut self, batch_max: Option<usize>) -> Self {
        self.batch_max = batch_max;
        self
    }

    /// Embed the whole batch, honouring the configured cap.
    ///
    /// Returns one vector per input text, in input order.
    async fn embed_all(&self, batch: &[String]) -> Result<Vec<Vec<f32>>, SelectorErrorKind> {
        match self.batch_max {
            Some(cap) if cap > 0 && batch.len() > cap => {
                let mut vectors = Vec::with_capacity(batch.len());
                for window in batch.chunks(cap) {
                    vectors.extend(
                        self.embedding
                            .embed_batch(window)
                            .await
                            .map_err(embedding_error)?,
                    );
                }
                Ok(vectors)
            },
            _ => self
                .embedding
                .embed_batch(batch)
                .await
                .map_err(embedding_error),
        }
    }
}

impl SemanticInspectorPort for EmbeddingSemanticInspector {
    fn find_semantic_match<'a>(
        &'a self,
        ctx: SemanticContext,
    ) -> BoxFuture<'a, Result<Option<SemanticMatch>, SelectorErrorKind>> {
        let span = info_span!(
            "semantic_inspector",
            fragments = ctx.dom_fragments.len(),
            threshold = self.threshold,
            batch_max = ?self.batch_max,
        );
        Box::pin(
            async move {
                if ctx.dom_fragments.is_empty() {
                    debug!("no dom fragments to compare against");
                    return Ok(None);
                }

                // One batch: target first, then the fragments in DOM order. The
                // vector at index i is the embedding of the text at index i,
                // which is what keeps the ranking below positional.
                let mut batch = Vec::with_capacity(ctx.dom_fragments.len() + 1);
                batch.push(ctx.target_text);
                batch.extend(ctx.dom_fragments);

                let vectors = self.embed_all(&batch).await?;
                if vectors.len() != batch.len() {
                    // Index-blind ranking would attribute a vector to the wrong
                    // fragment, so refuse the whole batch instead.
                    return Err(SelectorErrorKind::InvalidSelector(format!(
                        "embedding port returned {} vectors for {} inputs",
                        vectors.len(),
                        batch.len()
                    )));
                }

                let target = &vectors[0];
                let mut best: Option<(f32, &String)> = None;
                let mut skipped = 0usize;

                for (i, vector) in vectors.iter().enumerate().skip(1) {
                    // `None` = dimensions disagree: skip, never rank as 0.0.
                    let Some(score) = cosine_similarity_checked(target, vector) else {
                        skipped += 1;
                        continue;
                    };
                    if best.is_none_or(|(top, _)| score > top) {
                        best = Some((score, &batch[i]));
                    }
                }

                debug!(
                    best_score = best.map_or(0.0, |(score, _)| score),
                    skipped_mismatched = skipped,
                    threshold = self.threshold,
                    "semantic match evaluated"
                );

                match best {
                    Some((score, selector)) if score >= self.threshold => Ok(Some(SemanticMatch {
                        selector: selector.clone(),
                        confidence: score,
                        source: TierSource::Semantic,
                    })),
                    _ => Ok(None),
                }
            }
            .instrument(span),
        )
    }
}

/// Map a port failure onto the inspector's error surface.
fn embedding_error(err: SemanticError) -> SelectorErrorKind {
    SelectorErrorKind::InvalidSelector(format!("embedding failed: {err}"))
}

/// Cosine similarity, fail-closed on a dimension mismatch.
///
/// Returns `Some(score)` in `[-1.0, 1.0]`; `Some(0.0)` for empty or
/// zero-magnitude vectors, since those are a real absence of signal rather than
/// a broken contract. Returns `None` when the two vectors have different
/// lengths — the caller must SKIP that vector so a corrupt one can never rank,
/// which is the invariant the truncating variant (`embedding_ops::cosine_similarity`,
/// `a.len().min(b.len())`) breaks.
///
/// Local by design: `application::vault_search` carries an equivalent helper,
/// but importing a private `fn` from `application/` into `infrastructure/`
/// is an outward reference and the intra-crate gate forbids it.
fn cosine_similarity_checked(a: &[f32], b: &[f32]) -> Option<f32> {
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
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Deterministic embedding port with call accounting.
    ///
    /// `table` maps an exact text to the vector it must embed to; anything
    /// else falls back to a one-hot of width `fallback_dim` at
    /// `text.len() % fallback_dim`. `drop_last` makes `embed_batch` return one
    /// vector short of its input, which no honest port does — it exists so the
    /// short-output guard has a test.
    #[derive(Default)]
    struct MockEmbedder {
        table: HashMap<String, Vec<f32>>,
        fallback_dim: usize,
        drop_last: bool,
        embed_calls: AtomicUsize,
        batch_calls: AtomicUsize,
        batches: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl MockEmbedder {
        fn new(fallback_dim: usize, table: &[(&str, &[f32])]) -> Self {
            Self {
                table: table
                    .iter()
                    .map(|(text, vec)| ((*text).to_owned(), (*vec).to_vec()))
                    .collect(),
                fallback_dim,
                ..Self::default()
            }
        }

        fn vector_for(&self, text: &str) -> Vec<f32> {
            if let Some(vector) = self.table.get(text) {
                return vector.clone();
            }
            let mut vector = vec![0.0f32; self.fallback_dim];
            if !vector.is_empty() {
                vector[text.len() % self.fallback_dim] = 1.0;
            }
            vector
        }

        fn embed_calls(&self) -> usize {
            self.embed_calls.load(Ordering::SeqCst)
        }

        fn batch_calls(&self) -> usize {
            self.batch_calls.load(Ordering::SeqCst)
        }

        fn batched_texts(&self) -> Vec<Vec<String>> {
            self.batches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
        }
    }

    impl EmbeddingPort for MockEmbedder {
        fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
            self.embed_calls.fetch_add(1, Ordering::SeqCst);
            let vector = self.vector_for(text);
            Box::pin(async move { Ok(vector) })
        }

        fn embed_batch<'a>(
            &'a self,
            texts: &'a [String],
        ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
            self.batch_calls.fetch_add(1, Ordering::SeqCst);
            self.batches
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(texts.to_vec());
            let mut vectors: Vec<Vec<f32>> = texts.iter().map(|t| self.vector_for(t)).collect();
            if self.drop_last {
                vectors.pop();
            }
            Box::pin(async move { Ok(vectors) })
        }

        fn embedding_dim(&self) -> usize {
            self.fallback_dim
        }
    }

    fn context(target: &str, fragments: &[&str]) -> SemanticContext {
        SemanticContext {
            target_text: target.to_owned(),
            dom_fragments: fragments.iter().map(|f| (*f).to_owned()).collect(),
            domain_hint: None,
        }
    }

    /// Erase the mock for the inspector while the test keeps a handle to count
    /// its calls. `Arc::clone` would pick the generic signature and block the
    /// unsized coercion, so the clone is a plain method call on the argument.
    fn inspector_over(embedder: &Arc<MockEmbedder>, threshold: f32) -> EmbeddingSemanticInspector {
        EmbeddingSemanticInspector::new(embedder.clone(), threshold)
    }

    #[tokio::test]
    async fn match_above_threshold_returns_the_best_fragment() {
        let embedder = MockEmbedder::new(2, &[("target", &[1.0, 0.0]), ("match", &[1.0, 0.0])]);
        let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

        let hit = inspector
            .find_semantic_match(context("target", &["other", "match"]))
            .await
            .expect("inspection must not error")
            .expect("cosine 1.0 clears the 0.75 threshold");

        assert_eq!(hit.selector, "match");
        assert!(
            (hit.confidence - 1.0).abs() < 1e-5,
            "got {}",
            hit.confidence
        );
        assert_eq!(hit.source, TierSource::Semantic);
    }

    #[tokio::test]
    async fn no_match_below_threshold() {
        let embedder = MockEmbedder::new(2, &[("target", &[1.0, 0.0]), ("other", &[0.0, 1.0])]);
        let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

        let result = inspector
            .find_semantic_match(context("target", &["other"]))
            .await
            .expect("inspection must not error");

        assert_eq!(result, None, "cosine 0.0 must not clear the threshold");
    }

    /// The regression this whole slice exists for: a dimension mismatch used to
    /// be truncated to the shorter vector, so a 2-wide fragment identical to a
    /// 4-wide target scored a perfect 1.0 and WON. Fail-closed means the
    /// mismatch can never produce a match.
    #[tokio::test]
    async fn dimension_mismatch_is_skipped_never_scored_as_a_match() {
        let embedder = MockEmbedder::new(
            4,
            &[("target", &[1.0, 0.0, 0.0, 0.0]), ("short", &[1.0, 0.0])],
        );
        let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

        let result = inspector
            .find_semantic_match(context("target", &["short"]))
            .await
            .expect("a mismatch degrades, it does not fail the call");

        assert_eq!(
            result, None,
            "a 2-wide fragment must not be truncated into a match against a 4-wide target"
        );
    }

    /// Triangulates the skip policy: one broken vector must not discard the
    /// candidates that ARE comparable.
    #[tokio::test]
    async fn dimension_mismatch_does_not_discard_comparable_fragments() {
        let embedder = MockEmbedder::new(
            4,
            &[
                ("target", &[1.0, 0.0, 0.0, 0.0]),
                ("match", &[1.0, 0.0, 0.0, 0.0]),
                ("short", &[1.0, 0.0]),
            ],
        );
        let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

        let hit = inspector
            .find_semantic_match(context("target", &["short", "match"]))
            .await
            .expect("inspection must not error")
            .expect("the comparable fragment still matches");

        assert_eq!(hit.selector, "match");
    }

    #[tokio::test]
    async fn empty_dom_fragments_returns_none_without_calling_the_port() {
        let embedder = Arc::new(MockEmbedder::new(2, &[]));
        let inspector = inspector_over(&embedder, 0.75);

        let result = inspector
            .find_semantic_match(context("target", &[]))
            .await
            .expect("inspection must not error");

        assert_eq!(result, None);
        assert_eq!(
            embedder.batch_calls(),
            0,
            "an empty candidate set must not reach the embedding port"
        );
        assert_eq!(embedder.embed_calls(), 0);
    }

    #[tokio::test]
    async fn batch_order_is_preserved() {
        // Reversed embedding order would attribute "first" to "third".
        let embedder = MockEmbedder::new(
            2,
            &[
                ("target", &[1.0, 0.0]),
                ("first", &[0.0, 1.0]),
                ("second", &[0.0, 1.0]),
                ("third", &[1.0, 0.0]),
            ],
        );
        let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

        let hit = inspector
            .find_semantic_match(context("target", &["first", "second", "third"]))
            .await
            .expect("inspection must not error")
            .expect("only 'third' is parallel to the target");

        assert_eq!(hit.selector, "third");
    }

    #[tokio::test]
    async fn one_embed_batch_replaces_per_fragment_inference() {
        let embedder = Arc::new(MockEmbedder::new(2, &[("target", &[1.0, 0.0])]));
        let inspector = inspector_over(&embedder, 0.75);

        inspector
            .find_semantic_match(context("target", &["a", "b", "c", "d", "e"]))
            .await
            .expect("inspection must not error");

        assert_eq!(
            embedder.batch_calls(),
            1,
            "target + 5 fragments is ONE round trip, not N+1"
        );
        assert_eq!(
            embedder.embed_calls(),
            0,
            "the per-fragment path this slice removes must not run"
        );
        assert_eq!(
            embedder.batched_texts(),
            vec![vec![
                "target".to_owned(),
                "a".to_owned(),
                "b".to_owned(),
                "c".to_owned(),
                "d".to_owned(),
                "e".to_owned(),
            ]]
        );
    }

    #[tokio::test]
    async fn batch_max_splits_into_capped_windows_and_keeps_order() {
        let embedder = Arc::new(MockEmbedder::new(
            2,
            &[
                ("target", &[1.0, 0.0]),
                ("first", &[0.0, 1.0]),
                ("second", &[0.0, 1.0]),
                ("third", &[1.0, 0.0]),
            ],
        ));
        let inspector = inspector_over(&embedder, 0.75).with_batch_max(Some(2));

        let hit = inspector
            .find_semantic_match(context("target", &["first", "second", "third"]))
            .await
            .expect("inspection must not error")
            .expect("windowed batching must rank exactly like a single batch");

        assert_eq!(hit.selector, "third");
        assert_eq!(
            embedder.batched_texts(),
            vec![
                vec!["target".to_owned(), "first".to_owned()],
                vec!["second".to_owned(), "third".to_owned()],
            ],
            "windows must be consecutive and cap-sized, reassembled by position"
        );
    }

    #[tokio::test]
    async fn short_port_output_is_an_error_not_a_wrong_ranking() {
        let embedder = MockEmbedder {
            drop_last: true,
            ..MockEmbedder::new(2, &[("target", &[1.0, 0.0])])
        };
        let inspector = EmbeddingSemanticInspector::new(Arc::new(embedder), 0.75);

        let result = inspector
            .find_semantic_match(context("target", &["first", "second"]))
            .await;

        assert!(
            result.is_err(),
            "a vector count that does not match the input must not be ranked"
        );
    }

    #[test]
    fn cosine_similarity_checked_reports_mismatch_as_none() {
        assert_eq!(
            cosine_similarity_checked(&[1.0, 0.0], &[1.0, 0.0]),
            Some(1.0)
        );
        assert_eq!(
            cosine_similarity_checked(&[1.0, 0.0], &[0.0, 1.0]),
            Some(0.0)
        );
        assert_eq!(cosine_similarity_checked(&[], &[]), Some(0.0));
        assert_eq!(
            cosine_similarity_checked(&[0.0, 0.0], &[1.0, 0.0]),
            Some(0.0)
        );
        assert_eq!(cosine_similarity_checked(&[1.0, 0.0], &[1.0]), None);
        assert_eq!(
            cosine_similarity_checked(&[1.0, 0.0], &[1.0, 0.0, 0.0]),
            None
        );
    }

    /// Positive control for the skip policy: equal widths with a real score are
    /// compared, never rejected.
    #[test]
    fn equal_width_vectors_are_compared_even_when_not_parallel() {
        let score = cosine_similarity_checked(&[1.0, 1.0], &[1.0, 0.0])
            .expect("equal widths must produce a score, not a skip");
        assert!(
            (score - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5,
            "got {score}"
        );
    }
}
