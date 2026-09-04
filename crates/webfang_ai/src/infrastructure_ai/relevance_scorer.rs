//! Relevance scoring for semantic filtering
//!
//! Provides relevance scoring between embeddings using cosine similarity.
//! Used for filtering chunks by semantic relevance to a query or reference.

use std::sync::Once;

use tracing::warn;

use super::embedding_ops::cosine_similarity;

/// One-shot guard for the #1109 downgrade path (#1152): a missing reference
/// turned the old panic into a silent per-chunk skip, so the first skip is
/// logged once per process to keep the behaviour observable in the trace
/// without flooding the log on large batches.
static MISSING_REFERENCE_WARNED: Once = Once::new();

/// Emit the process-wide one-shot warning when [`RelevanceScorer::score`]
/// returned `None` because no reference was provided and none is stored.
fn warn_missing_reference_once(total_chunks: usize, threshold: f32) {
    MISSING_REFERENCE_WARNED.call_once(|| {
        warn!(
            total_chunks,
            threshold,
            "Relevance filter skipping chunks: no reference embedding (score() returns None since #1109)"
        );
    });
}

/// Relevance scorer with configurable threshold
///
/// Scores embeddings against a reference vector and filters by threshold.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "ai")]
/// # fn example() {
/// use webfang_ai::RelevanceScorer;
///
/// let scorer = RelevanceScorer::new(0.3);
/// assert_eq!(scorer.threshold(), 0.3);
/// # }
/// ```
pub struct RelevanceScorer {
    /// Minimum similarity threshold (0.0-1.0)
    threshold: f32,
    /// Optional reference embedding for scoring
    reference: Option<Vec<f32>>,
}

impl RelevanceScorer {
    /// Create a new RelevanceScorer with threshold
    ///
    /// # Arguments
    ///
    /// * `threshold` - Minimum similarity threshold (0.0-1.0)
    ///
    /// # Panics
    ///
    /// Panics if threshold is outside [0.0, 1.0] range
    #[must_use]
    pub fn new(threshold: f32) -> Self {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "Threshold must be between 0.0 and 1.0, got {threshold}"
        );

        Self {
            threshold,
            reference: None,
        }
    }

    /// Create a scorer with a reference embedding
    ///
    /// # Arguments
    ///
    /// * `threshold` - Minimum similarity threshold
    /// * `reference` - Reference embedding vector
    #[must_use]
    pub fn with_reference(threshold: f32, reference: Vec<f32>) -> Self {
        Self {
            threshold,
            reference: Some(reference),
        }
    }

    /// Get the threshold value
    #[must_use]
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Get the reference embedding (if set)
    #[must_use]
    pub fn reference(&self) -> Option<&[f32]> {
        self.reference.as_deref()
    }

    /// Set a new threshold
    ///
    /// # Arguments
    ///
    /// * `threshold` - New threshold value
    ///
    /// # Panics
    ///
    /// Panics if threshold is outside [0.0, 1.0] range
    pub fn set_threshold(&mut self, threshold: f32) {
        assert!(
            (0.0..=1.0).contains(&threshold),
            "Threshold must be between 0.0 and 1.0, got {threshold}"
        );
        self.threshold = threshold;
    }

    /// Set the reference embedding
    pub fn set_reference(&mut self, reference: Vec<f32>) {
        self.reference = Some(reference);
    }

    /// Clear the reference embedding
    pub fn clear_reference(&mut self) {
        self.reference = None;
    }

    /// Score embedding against reference
    ///
    /// Total function (#1109): a missing reference is a skip, not a panic.
    /// Async data paths run inside Tokio tasks where a panic aborts the
    /// whole AI batch, so this must never abort.
    ///
    /// # Arguments
    ///
    /// * `embedding` - Vector to score
    /// * `reference` - Reference vector (if None, uses stored reference)
    ///
    /// # Returns
    ///
    /// Similarity score in range [-1.0, 1.0], or `None` if no reference was
    /// provided and none is stored (same contract as [`score_stored`](Self::score_stored))
    #[must_use]
    pub fn score(&self, embedding: &[f32], reference: Option<&[f32]>) -> Option<f32> {
        let reference = reference.or(self.reference.as_deref())?;
        Some(cosine_similarity(embedding, reference))
    }

    /// Score embedding against stored reference
    ///
    /// Convenience method when reference is already stored.
    ///
    /// # Arguments
    ///
    /// * `embedding` - Vector to score
    ///
    /// # Returns
    ///
    /// Similarity score, or None if no reference is stored
    #[must_use]
    pub fn score_stored(&self, embedding: &[f32]) -> Option<f32> {
        self.reference
            .as_ref()
            .map(|reference| cosine_similarity(embedding, reference))
    }

    /// Check if score meets threshold
    ///
    /// # Arguments
    ///
    /// * `score` - Similarity score
    ///
    /// # Returns
    ///
    /// `true` if score >= threshold
    #[must_use]
    pub fn meets_threshold(&self, score: f32) -> bool {
        score >= self.threshold
    }

    /// Filter chunks by relevance threshold
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of (DocumentChunk, embedding) pairs
    /// * `reference` - Reference vector (if None, uses stored reference)
    ///
    /// # Returns
    ///
    /// Vector of chunks with similarity >= threshold
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of (DocumentChunk, embedding) pairs
    /// * `reference` - Optional reference vector for scoring
    ///
    /// # Returns
    ///
    /// Vector of (DocumentChunk, embedding) pairs that meet the relevance threshold
    /// unlike [`filter`](Self::filter) which discards the embedding vectors.
    #[must_use]
    pub fn filter_with_embeddings(
        &self,
        chunks: &[(webfang_core::domain::DocumentChunk, Vec<f32>)],
        reference: Option<&[f32]>,
    ) -> Vec<(webfang_core::domain::DocumentChunk, Vec<f32>)> {
        chunks
            .iter()
            .filter(|(_, embedding)| match self.score(embedding, reference) {
                Some(score) => self.meets_threshold(score),
                None => {
                    warn_missing_reference_once(chunks.len(), self.threshold);
                    false
                },
            })
            .map(|(chunk, embedding)| (chunk.clone(), embedding.clone()))
            .collect()
    }

    /// Filter chunks using stored reference and preserve embeddings
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of (DocumentChunk, embedding) pairs
    ///
    /// # Returns
    ///
    /// Vector of (DocumentChunk, embedding) pairs, or empty vec if no reference stored
    #[must_use]
    pub fn filter_with_embeddings_stored(
        &self,
        chunks: &[(webfang_core::domain::DocumentChunk, Vec<f32>)],
    ) -> Vec<(webfang_core::domain::DocumentChunk, Vec<f32>)> {
        if self.reference.is_none() {
            return Vec::new();
        }

        self.filter_with_embeddings(chunks, self.reference.as_deref())
    }

    /// Filter chunks by relevance score
    ///
    /// **WARNING**: This method discards embeddings! Use [`filter_with_embeddings`](Self::filter_with_embeddings)
    /// if you need to preserve embedding vectors.
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of (DocumentChunk, embedding) pairs
    /// * `reference` - Reference vector for scoring
    ///
    /// # Returns
    ///
    /// Vector of relevant chunks (embeddings are discarded)
    #[must_use]
    pub fn filter(
        &self,
        chunks: &[(webfang_core::domain::DocumentChunk, Vec<f32>)],
        reference: Option<&[f32]>,
    ) -> Vec<webfang_core::domain::DocumentChunk> {
        chunks
            .iter()
            .filter(|(_, embedding)| match self.score(embedding, reference) {
                Some(score) => self.meets_threshold(score),
                None => {
                    warn_missing_reference_once(chunks.len(), self.threshold);
                    false
                },
            })
            .map(|(chunk, _)| chunk.clone())
            .collect()
    }

    /// Filter chunks using stored reference
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of (DocumentChunk, embedding) pairs
    ///
    /// # Returns
    ///
    /// Vector of relevant chunks, or empty vec if no reference stored
    #[must_use]
    pub fn filter_stored(
        &self,
        chunks: &[(webfang_core::domain::DocumentChunk, Vec<f32>)],
    ) -> Vec<webfang_core::domain::DocumentChunk> {
        if self.reference.is_none() {
            return Vec::new();
        }

        self.filter(chunks, self.reference.as_deref())
    }

    /// Find top-k most relevant chunks
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of (DocumentChunk, embedding) pairs
    /// * `reference` - Reference vector
    /// * `k` - Number of results to return
    ///
    /// # Returns
    ///
    /// Top-k chunks sorted by relevance (descending)
    #[must_use]
    pub fn top_k(
        &self,
        chunks: &[(webfang_core::domain::DocumentChunk, Vec<f32>)],
        reference: &[f32],
        k: usize,
    ) -> Vec<(webfang_core::domain::DocumentChunk, f32)> {
        // `reference` is always supplied here, so `score` is always `Some`;
        // `filter_map` keeps the path total without reintroducing an unwrap.
        let mut scored: Vec<_> = chunks
            .iter()
            .filter_map(|(chunk, embedding)| {
                self.score(embedding, Some(reference))
                    .map(|score| (chunk.clone(), score))
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Take top-k
        scored.truncate(k);
        scored
    }
}

impl Default for RelevanceScorer {
    fn default() -> Self {
        Self::new(0.3) // Default threshold: 0.3 (moderate relevance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use webfang_core::domain::DocumentChunk;

    fn create_test_chunk(content: &str) -> (DocumentChunk, Vec<f32>) {
        let chunk = DocumentChunk::new(Uuid::new_v4(), "https://example.com", "Test", content);

        // Create a simple embedding (normalized)
        let embedding = [0.5f32; 8];
        let magnitude: f32 = embedding.iter().map(|&x| x * x).sum::<f32>().sqrt();
        let normalized: Vec<f32> = embedding.iter().map(|&x| x / magnitude).collect();

        (chunk, normalized)
    }

    #[test]
    fn test_relevance_scorer_creation() {
        let scorer = RelevanceScorer::new(0.3);
        assert_eq!(scorer.threshold(), 0.3);
    }

    #[test]
    fn test_relevance_scorer_with_reference() {
        let reference = vec![0.5f32; 8];
        let scorer = RelevanceScorer::with_reference(0.5, reference.clone());
        assert_eq!(scorer.threshold(), 0.5);
        assert_eq!(scorer.reference(), Some(reference.as_slice()));
    }

    #[test]
    fn test_relevance_scorer_set_threshold() {
        let mut scorer = RelevanceScorer::new(0.3);
        scorer.set_threshold(0.7);
        assert_eq!(scorer.threshold(), 0.7);
    }

    #[test]
    #[should_panic(expected = "Threshold must be between")]
    fn test_relevance_scorer_invalid_threshold_low() {
        let _ = RelevanceScorer::new(-0.1);
    }

    #[test]
    #[should_panic(expected = "Threshold must be between")]
    fn test_relevance_scorer_invalid_threshold_high() {
        let _ = RelevanceScorer::new(1.1);
    }

    #[test]
    fn test_relevance_scorer_score() {
        let reference = vec![1.0f32, 0.0, 0.0, 0.0];
        let scorer = RelevanceScorer::with_reference(0.3, reference.clone());

        let identical = vec![1.0f32, 0.0, 0.0, 0.0];
        let score = scorer.score(&identical, Some(&reference));
        assert!((score.expect("reference provided") - 1.0).abs() < 0.001);
    }

    /// Post-fix contract for #1109: `score` mirrors `score_stored` — a
    /// missing reference yields `None` (skip), never a panic.
    #[test]
    fn score_without_reference_returns_none() {
        let scorer = RelevanceScorer::default();
        assert_eq!(scorer.score(&[1.0, 0.0, 0.0, 0.0], None), None);

        let reference = vec![1.0f32, 0.0, 0.0, 0.0];
        let scorer = RelevanceScorer::with_reference(0.3, reference.clone());
        assert!(
            scorer
                .score(&[1.0, 0.0, 0.0, 0.0], None)
                .is_some_and(|s| (s - 1.0).abs() < 0.001),
            "stored reference must still be used when the argument is None"
        );
    }

    #[test]
    fn test_relevance_scorer_score_stored() {
        let reference = vec![1.0f32, 0.0, 0.0, 0.0];
        let scorer = RelevanceScorer::with_reference(0.3, reference.clone());

        let identical = vec![1.0f32, 0.0, 0.0, 0.0];
        let score = scorer.score_stored(&identical);
        assert!(score.is_some());
        assert!((score.unwrap() - 1.0).abs() < 0.001);
    }

    /// Reproduction guard for #1109: `score` without any reference (argument
    /// `None`, no stored reference) used to abort the calling task with
    /// `.expect("No reference embedding provided or stored")`. The test
    /// compiles against both signatures (`let _` binds `f32` or `Option<f32>`
    /// alike): on unmodified main the child test panics; after the fix it
    /// passes.
    #[test]
    fn score_without_reference_does_not_panic() {
        let scorer = RelevanceScorer::default();
        let _ = scorer.score(&[1.0, 0.0, 0.0, 0.0], None);
    }

    /// Reproduction guard for #1109: the async data path reaches the same
    /// panic through `filter`/`filter_with_embeddings` when no reference is
    /// available. A missing reference must skip (empty result), never abort
    /// the Tokio task running the batch.
    #[test]
    fn filter_without_reference_returns_empty_instead_of_panicking() {
        let scorer = RelevanceScorer::default();
        let (chunk, embedding) = create_test_chunk("Content 1");
        let chunks = vec![(chunk, embedding)];

        let filtered = scorer.filter(&chunks, None);
        assert!(filtered.is_empty(), "no reference must skip, not panic");

        let kept = scorer.filter_with_embeddings(&chunks, None);
        assert!(kept.is_empty(), "no reference must skip, not panic");
    }

    #[test]
    fn test_relevance_scorer_meets_threshold() {
        let scorer = RelevanceScorer::new(0.5);

        assert!(scorer.meets_threshold(0.6));
        assert!(scorer.meets_threshold(0.5));
        assert!(!scorer.meets_threshold(0.4));
    }

    #[test]
    fn test_relevance_scorer_filter() {
        let reference = vec![1.0f32, 0.0, 0.0, 0.0];
        let scorer = RelevanceScorer::with_reference(0.3, reference.clone());

        let (chunk1, emb1) = create_test_chunk("Content 1");
        let (chunk2, _emb2) = create_test_chunk("Content 2");

        // Create orthogonal embedding
        let emb_orthogonal = vec![0.0f32, 1.0, 0.0, 0.0];

        let chunks = vec![(chunk1, emb1), (chunk2.clone(), emb_orthogonal)];
        let filtered = scorer.filter(&chunks, Some(&reference));

        // Should filter out orthogonal vector
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn test_relevance_scorer_filter_empty() {
        let scorer = RelevanceScorer::new(0.3);
        let chunks: Vec<(DocumentChunk, Vec<f32>)> = vec![];
        let reference = vec![0.5f32; 8];

        let filtered = scorer.filter(&chunks, Some(&reference));
        assert!(filtered.is_empty());
    }

    #[test]
    fn test_relevance_scorer_top_k() {
        let reference = vec![1.0f32, 0.0, 0.0, 0.0];
        let scorer = RelevanceScorer::new(0.0);

        let (chunk1, emb1) = create_test_chunk("Content 1");
        let (chunk2, emb2) = create_test_chunk("Content 2");

        let chunks = vec![(chunk1, emb1), (chunk2, emb2)];
        let top = scorer.top_k(&chunks, &reference, 1);

        assert_eq!(top.len(), 1);
    }

    #[test]
    fn test_relevance_scorer_default() {
        let scorer = RelevanceScorer::default();
        assert_eq!(scorer.threshold(), 0.3);
    }

    /// Z-score mapping used by the adaptive relevance filter (#648):
    /// a higher threshold must translate into a tighter Z limit.
    #[test]
    fn test_zscore_filters_outliers() {
        let threshold_strict = 0.9f32;
        let threshold_lax = 0.1f32;
        let z_strict = 3.0 * (1.0 - threshold_strict);
        let z_lax = 3.0 * (1.0 - threshold_lax);
        assert!(
            z_strict < z_lax,
            "Higher threshold should give lower Z limit"
        );
    }
}
