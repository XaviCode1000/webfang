//! Relevance scoring for semantic filtering
//!
//! Provides relevance scoring between embeddings using cosine similarity.
//! Used for filtering chunks by semantic relevance to a query or reference.

use super::embedding_ops::cosine_similarity;

/// Relevance scorer with configurable threshold
///
/// Scores embeddings against a reference vector and filters by threshold.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "ai")]
/// # fn example() {
/// use rust_scraper::infrastructure::ai::RelevanceScorer;
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
            "Threshold must be between 0.0 and 1.0, got {}",
            threshold
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
            "Threshold must be between 0.0 and 1.0, got {}",
            threshold
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
    /// # Arguments
    ///
    /// * `embedding` - Vector to score
    /// * `reference` - Reference vector (if None, uses stored reference)
    ///
    /// # Returns
    ///
    /// Similarity score in range [-1.0, 1.0]
    ///
    /// # Panics
    ///
    /// Panics if no reference is provided and none is stored
    #[must_use]
    pub fn score(&self, embedding: &[f32], reference: Option<&[f32]>) -> f32 {
        let reference = reference
            .or(self.reference.as_deref())
            .expect("No reference embedding provided or stored");

        cosine_similarity(embedding, reference)
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
    /// # Examples
    ///
    /// ```no_run
    /// # #[cfg(feature = "ai")]
    /// Filter chunks and preserve their embeddings
    ///
    /// Unlike [`filter`](Self::filter), this method returns the chunks WITH their
    /// embedding vectors, not just the chunks.
    ///
    /// # Arguments
    ///
    /// * `chunks` - Slice of (DocumentChunk, embedding) pairs
    /// * `reference` - Optional reference vector for scoring
    ///
    /// # Returns
    ///
    /// Vector of (DocumentChunk, embedding) pairs that meet the relevance threshold
    #[must_use]
    pub fn filter_with_embeddings(
        &self,
        chunks: &[(rust_scraper_core::domain::DocumentChunk, Vec<f32>)],
        reference: Option<&[f32]>,
    ) -> Vec<(rust_scraper_core::domain::DocumentChunk, Vec<f32>)> {
        chunks
            .iter()
            .filter(|(_, embedding)| {
                let score = self.score(embedding, reference);
                self.meets_threshold(score)
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
        chunks: &[(rust_scraper_core::domain::DocumentChunk, Vec<f32>)],
    ) -> Vec<(rust_scraper_core::domain::DocumentChunk, Vec<f32>)> {
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
    ///
    /// # fn example() -> anyhow::Result<()> {
    /// use rust_scraper::infrastructure::ai::RelevanceScorer;
    /// use rust_scraper::domain::DocumentChunk;
    ///
    /// let scorer = RelevanceScorer::new(0.3);
    /// let reference = vec![0.1f32; 384]; // all-MiniLM-L6-v2 dimension
    ///
    /// // Example chunks with embeddings
    /// let chunks: Vec<(DocumentChunk, Vec<f32>)> = vec![];
    ///
    /// let filtered = scorer.filter(&chunks, Some(&reference));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn filter(
        &self,
        chunks: &[(rust_scraper_core::domain::DocumentChunk, Vec<f32>)],
        reference: Option<&[f32]>,
    ) -> Vec<rust_scraper_core::domain::DocumentChunk> {
        chunks
            .iter()
            .filter(|(_, embedding)| {
                let score = self.score(embedding, reference);
                self.meets_threshold(score)
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
        chunks: &[(rust_scraper_core::domain::DocumentChunk, Vec<f32>)],
    ) -> Vec<rust_scraper_core::domain::DocumentChunk> {
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
        chunks: &[(rust_scraper_core::domain::DocumentChunk, Vec<f32>)],
        reference: &[f32],
        k: usize,
    ) -> Vec<(rust_scraper_core::domain::DocumentChunk, f32)> {
        let mut scored: Vec<_> = chunks
            .iter()
            .map(|(chunk, embedding)| {
                let score = self.score(embedding, Some(reference));
                (chunk.clone(), score)
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
    use rust_scraper_core::domain::DocumentChunk;
    use uuid::Uuid;

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
        assert!((score - 1.0).abs() < 0.001);
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
}
