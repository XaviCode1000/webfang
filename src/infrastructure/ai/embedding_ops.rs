//! SIMD-accelerated embedding operations
//!
//! Provides high-performance vector operations for embedding processing:
//! - Cosine similarity using AVX2 SIMD (`opt-simd-portable`)
//! - Dot product for normalized vectors
//! - Batch operations for efficiency
//!
//! # Performance
//!
//! On Haswell (AVX2), `wide::f32x8` provides 4-8x speedup over scalar operations.
//! The `wide` crate is used for stable SIMD without nightly Rust.

use wide::f32x8;

/// SIMD-accelerated cosine similarity
///
/// Computes cosine similarity between two vectors using AVX2 SIMD instructions.
///
/// # Mathematical Background
///
/// For normalized vectors (unit length), cosine similarity equals dot product:
/// ```text
/// cos(θ) = (A · B) / (||A|| × ||B||)
/// ```
///
/// When `||A|| = ||B|| = 1` (normalized):
/// ```text
/// cos(θ) = A · B = Σ(aᵢ × bᵢ)
/// ```
///
/// The `all-MiniLM-L6-v2` model outputs normalized embeddings, so we can use
/// dot product directly.
///
/// # Arguments
///
/// * `a` - First vector (should be normalized)
/// * `b` - Second vector (should be normalized)
///
/// # Returns
///
/// Cosine similarity in range [-1.0, 1.0]:
/// - `1.0`: Identical vectors
/// - `0.0`: Orthogonal (unrelated)
/// - `-1.0`: Opposite vectors
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "ai")]
/// # fn example() {
/// use rust_scraper::infrastructure::ai::embedding_ops::cosine_similarity;
///
/// // Identical vectors
/// let vec = vec![0.5f32, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
/// let sim = cosine_similarity(&vec, &vec);
/// assert!((sim - 1.0).abs() < 0.001);
///
/// // Orthogonal vectors
/// let a = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
/// let b = vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
/// let sim = cosine_similarity(&a, &b);
/// assert!(sim.abs() < 0.001);
/// # }
/// ```
///
/// # Performance Notes
///
/// - Uses `wide::f32x8` for 8-wide SIMD parallelism
/// - Processes 8 floats per instruction on Haswell (AVX2)
/// - Falls back to scalar for remainder elements
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());

    if len == 0 {
        return 0.0;
    }

    // Process 8 elements at a time using wide::f32x8
    let simd_chunks = len / 8;
    let remainder = len % 8;

    // SIMD dot product using f32x8
    let mut sum = f32x8::splat(0.0);

    for i in 0..simd_chunks {
        let offset = i * 8;
        // Use array conversion (wide doesn't have from_slice)
        let mut av_array = [0.0f32; 8];
        let mut bv_array = [0.0f32; 8];
        av_array.copy_from_slice(&a[offset..offset + 8]);
        bv_array.copy_from_slice(&b[offset..offset + 8]);
        let av = f32x8::from(av_array);
        let bv = f32x8::from(bv_array);
        sum += av * bv;
    }

    // Reduce SIMD lanes to scalar using reduce_add
    let mut dot_product = sum.reduce_add();

    // Handle remainder elements (scalar fallback)
    let scalar_start = simd_chunks * 8;
    for i in scalar_start..scalar_start + remainder {
        dot_product += a[i] * b[i];
    }

    dot_product
}

/// Compute dot product of two vectors (scalar fallback)
///
/// Used when vectors are too small for SIMD or as a reference implementation.
///
/// # Arguments
///
/// * `a` - First vector
/// * `b` - Second vector
///
/// # Returns
///
/// Dot product value
#[must_use]
pub fn dot_product_scalar(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    a[..len]
        .iter()
        .zip(b[..len].iter())
        .map(|(&x, &y)| x * y)
        .sum()
}

/// Normalize a vector to unit length
///
/// # Arguments
///
/// * `vector` - Input vector
///
/// # Returns
///
/// Normalized vector (unit length)
///
/// # Panics
///
/// Panics if the vector has zero magnitude
#[must_use]
pub fn normalize(vector: &[f32]) -> Vec<f32> {
    let magnitude = vector.iter().map(|&x| x * x).sum::<f32>().sqrt();

    if magnitude < f32::EPSILON {
        panic!("Cannot normalize zero-magnitude vector");
    }

    vector.iter().map(|&x| x / magnitude).collect()
}

/// Compute Euclidean distance between two vectors
///
/// # Arguments
///
/// * `a` - First vector
/// * `b` - Second vector
///
/// # Returns
///
/// Euclidean distance
#[must_use]
pub fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    a[..len]
        .iter()
        .zip(b[..len].iter())
        .map(|(&x, &y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

/// Batch cosine similarity
///
/// Compute similarity between one query vector and multiple candidate vectors.
///
/// # Arguments
///
/// * `query` - Query vector
/// * `candidates` - Slice of candidate vectors
///
/// # Returns
///
/// Vector of similarity scores
#[must_use]
pub fn batch_cosine_similarity(query: &[f32], candidates: &[Vec<f32>]) -> Vec<f32> {
    candidates
        .iter()
        .map(|candidate| cosine_similarity(query, candidate))
        .collect()
}

/// Find most similar vector from candidates
///
/// # Arguments
///
/// * `query` - Query vector
/// * `candidates` - Slice of candidate vectors
///
/// # Returns
///
/// Index of most similar candidate, or None if empty
#[must_use]
pub fn find_most_similar(query: &[f32], candidates: &[Vec<f32>]) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }

    let mut best_idx = 0;
    let mut best_score = f32::NEG_INFINITY;

    for (idx, candidate) in candidates.iter().enumerate() {
        let score = cosine_similarity(query, candidate);
        if score > best_score {
            best_score = score;
            best_idx = idx;
        }
    }

    Some(best_idx)
}

/// Attention-mask weighted Mean Pooling
///
/// Computes the mean of token embeddings weighted by the attention mask.
/// This is the standard pooling strategy for sentence-transformers models.
///
/// # Arguments
///
/// * `token_embeddings` - Flat slice of token embeddings [seq_len * embedding_dim]
/// * `seq_len` - Number of tokens in sequence
/// * `embedding_dim` - Dimension of each embedding (typically 384 for all-MiniLM-L6-v2)
/// * `attention_mask` - slice of 0/1 values, length must equal seq_len
///
/// # Returns
///
/// Pooled embedding vector of length `embedding_dim`
///
/// # Examples
///
/// ```
/// # use rust_scraper::infrastructure::ai::embedding_ops::mean_pool;
/// let data: Vec<f32> = (0..4 * 384).map(|i| i as f32).collect();
/// let pooled = mean_pool(&data, 4, 384, &[1i64; 4]);
/// assert_eq!(pooled.len(), 384);
/// ```
#[must_use]
pub fn mean_pool(
    token_embeddings: &[f32],
    seq_len: usize,
    embedding_dim: usize,
    attention_mask: &[i64],
) -> Vec<f32> {
    debug_assert_eq!(
        attention_mask.len(),
        seq_len,
        "attention_mask length must match seq_len"
    );
    debug_assert_eq!(
        token_embeddings.len(),
        seq_len * embedding_dim,
        "token_embeddings length must match seq_len * embedding_dim"
    );

    let mut pooled = vec![0.0f32; embedding_dim];
    let mut mask_sum = 0.0f32;

    for (i, &weight) in attention_mask.iter().take(seq_len).enumerate() {
        let weight_f = weight as f32;
        if weight_f > 0.0 {
            mask_sum += weight_f;
            let offset = i * embedding_dim;
            for j in 0..embedding_dim {
                pooled[j] += token_embeddings[offset + j] * weight_f;
            }
        }
    }

    if mask_sum > f32::EPSILON {
        let inv_mask_sum = 1.0 / mask_sum;
        for v in &mut pooled {
            *v *= inv_mask_sum;
        }
    }
    // If mask_sum == 0, pooled stays as zero vector (defensive)

    pooled
}

/// L2 normalize a vector, returning zero vector if magnitude is too small
///
/// Unlike `normalize()`, this function never panics. If the input vector
/// has near-zero magnitude, it returns the input unchanged.
///
/// # Arguments
///
/// * `vector` - Input vector to normalize
///
/// # Returns
///
/// Normalized vector (unit length) or original if magnitude < epsilon
///
/// # Examples
///
/// ```
/// # use rust_scraper::infrastructure::ai::embedding_ops::l2_normalize_safe;
/// let v = vec![3.0f32, 4.0];
/// let normalized = l2_normalize_safe(&v);
/// let mag: f32 = normalized.iter().map(|&x| x * x).sum::<f32>().sqrt();
/// assert!((mag - 1.0).abs() < 0.001);
///
/// let zero = vec![0.0f32, 0.0, 0.0];
/// let result = l2_normalize_safe(&zero);
/// assert_eq!(result, vec![0.0, 0.0, 0.0]); // No panic, returns zero
/// ```
#[must_use]
pub fn l2_normalize_safe(vector: &[f32]) -> Vec<f32> {
    let magnitude = vector.iter().map(|&x| x * x).sum::<f32>().sqrt();

    if magnitude < f32::EPSILON {
        return vector.to_vec();
    }

    vector.iter().map(|&x| x / magnitude).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        // Use a normalized vector (magnitude = 1.0)
        // 1/sqrt(8) ≈ 0.3536 for 8-dimensional unit vector
        let normalization = 1.0f32 / 8.0f32.sqrt();
        let vec = vec![normalization; 8];
        let sim = cosine_similarity(&vec, &vec);
        assert!((sim - 1.0).abs() < 0.001, "Expected ~1.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001, "Expected ~0.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let b = vec![-1.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 0.001, "Expected ~-1.0, got {}", sim);
    }

    #[test]
    fn test_cosine_similarity_partial() {
        let a = vec![1.0f32, 0.0, 0.0, 0.0];
        let b = vec![0.0f32, 0.0, 0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_dot_product_scalar() {
        let a = vec![1.0f32, 2.0, 3.0];
        let b = vec![4.0f32, 5.0, 6.0];
        let dot = dot_product_scalar(&a, &b);
        assert_eq!(dot, 32.0); // 1*4 + 2*5 + 3*6 = 32
    }

    #[test]
    fn test_normalize() {
        let v = vec![3.0f32, 4.0];
        let normalized = normalize(&v);
        let magnitude: f32 = normalized.iter().map(|&x| x * x).sum::<f32>().sqrt();
        assert!((magnitude - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_euclidean_distance() {
        let a = vec![0.0f32, 0.0];
        let b = vec![3.0f32, 4.0];
        let dist = euclidean_distance(&a, &b);
        assert!((dist - 5.0).abs() < 0.001); // 3-4-5 triangle
    }

    #[test]
    fn test_batch_cosine_similarity() {
        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        let candidates = vec![
            vec![1.0f32, 0.0, 0.0, 0.0],  // identical
            vec![0.0f32, 1.0, 0.0, 0.0],  // orthogonal
            vec![-1.0f32, 0.0, 0.0, 0.0], // opposite
        ];
        let scores = batch_cosine_similarity(&query, &candidates);
        assert!((scores[0] - 1.0).abs() < 0.001);
        assert!(scores[1].abs() < 0.001);
        assert!((scores[2] + 1.0).abs() < 0.001);
    }

    #[test]
    fn test_find_most_similar() {
        let query = vec![1.0f32, 0.0, 0.0, 0.0];
        let candidates = vec![
            vec![0.0f32, 1.0, 0.0, 0.0], // orthogonal
            vec![1.0f32, 0.0, 0.0, 0.0], // identical (best)
            vec![0.0f32, 0.0, 1.0, 0.0], // orthogonal
        ];
        let best_idx = find_most_similar(&query, &candidates);
        assert_eq!(best_idx, Some(1));
    }

    #[test]
    fn test_find_most_similar_empty() {
        let query = vec![1.0f32, 0.0, 0.0];
        let candidates: Vec<Vec<f32>> = vec![];
        let best_idx = find_most_similar(&query, &candidates);
        assert_eq!(best_idx, None);
    }

    #[test]
    fn test_normalize_panic() {
        let v = vec![0.0f32, 0.0, 0.0];
        let result = std::panic::catch_unwind(|| normalize(&v));
        assert!(result.is_err());
    }

    #[test]
    fn test_cosine_similarity_large_vector() {
        // Test with vector larger than 8 elements (tests SIMD + remainder)
        let a: Vec<f32> = (0..20).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let b: Vec<f32> = (0..20).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_mean_pool_produces_correct_dimension() {
        // Simulate [4, 384] output with all mask=1
        let data: Vec<f32> = (0..4 * 384).map(|i| i as f32).collect();
        let mask = vec![1i64; 4];
        let pooled = mean_pool(&data, 4, 384, &mask);
        assert_eq!(pooled.len(), 384);
    }

    #[test]
    fn test_mean_pool_excludes_padding() {
        // 2 real tokens + 2 padding (seq_len=4, embedding_dim=2)
        let data = vec![1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0];
        let mask = vec![1i64, 1, 0, 0];
        let pooled = mean_pool(&data, 4, 2, &mask);
        // Mean of first 2 rows: [(1+10)/2, (2+20)/2] = [5.5, 11.0]
        assert!((pooled[0] - 5.5).abs() < 0.1, "expected 5.5, got {}", pooled[0]);
        assert!((pooled[1] - 11.0).abs() < 0.1, "expected 11.0, got {}", pooled[1]);
    }

    #[test]
    fn test_mean_pool_empty_mask() {
        let data = vec![1.0, 2.0, 3.0, 4.0];
        let mask = vec![0i64, 0];
        let pooled = mean_pool(&data, 2, 2, &mask);
        assert_eq!(pooled, vec![0.0, 0.0]);
    }

    #[test]
    fn test_l2_normalize_safe_unit_magnitude() {
        let v = vec![3.0f32, 4.0];
        let normalized = l2_normalize_safe(&v);
        let mag: f32 = normalized.iter().map(|&x| x * x).sum::<f32>().sqrt();
        assert!((mag - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_l2_normalize_safe_zero_vector() {
        let v = vec![0.0f32, 0.0, 0.0];
        let result = l2_normalize_safe(&v);
        assert_eq!(result, vec![0.0, 0.0, 0.0]); // No panic, returns zero
    }

    #[test]
    fn test_mean_pool_single_token() {
        // Single token with mask=1
        let data = vec![1.0f32, 2.0, 3.0, 4.0];
        let mask = vec![1i64];
        let pooled = mean_pool(&data, 1, 4, &mask);
        assert_eq!(pooled.len(), 4);
        assert_eq!(pooled, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_mean_pool_with_l2_normalize_pipeline() {
        // Test the full pipeline: mean_pool + l2_normalize_safe
        let data: Vec<f32> = (0..4 * 384).map(|i| (i % 384) as f32).collect();
        let mask = vec![1i64; 4];

        let pooled = mean_pool(&data, 4, 384, &mask);
        let normalized = l2_normalize_safe(&pooled);

        assert_eq!(normalized.len(), 384);

        // Check L2 norm is ~1.0
        let mag: f32 = normalized.iter().map(|&x| x * x).sum::<f32>().sqrt();
        assert!((mag - 1.0).abs() < 0.001);
    }
}
