//! Vault search service — semantic search over Obsidian vault notes (#386).
//!
//! Orchestrates the search pipeline: load indexed vectors → embed query →
//! rank by cosine similarity → return top-N results. Also provides note
//! indexing: chunk markdown → embed chunks → persist to NoteRepository.
//!
//! # Architecture
//!
//! Concrete application struct (following the `ElasticIngestion` pattern).
//! Injects domain ports (`EmbeddingPort`, `NoteRepository`, `TextChunker`)
//! — no dependency on `webfang_ai` concrete types.

use std::sync::Arc;

use tracing::{debug, info, instrument};

use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::note_repository::NoteRepository;
use crate::domain::text_chunker::TextChunker;
use crate::error::ScraperError;

/// A search result with relevance score and source metadata.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct VaultSearchResult {
    /// Filesystem path of the source note.
    pub note_path: String,
    /// The matching chunk's text content.
    pub content: String,
    /// Cosine similarity score (0.0 to 1.0).
    pub score: f32,
    /// Zero-based chunk index within the note.
    pub chunk_index: i64,
    /// Heading context (if available from chunking metadata).
    pub heading: Option<String>,
}

/// Semantic search service for Obsidian vault notes.
///
/// Orchestrates domain ports to provide:
/// - **Search**: embed query → load indexed vectors → rank → top-N
/// - **Indexing**: chunk markdown → embed chunks → persist
///
/// Construct with [`VaultSearchService::new`]; all dependencies are
/// injected as `Arc<dyn Port>` for testability.
pub struct VaultSearchService {
    embedding: Arc<dyn EmbeddingPort>,
    repository: Arc<dyn NoteRepository>,
    chunker: Arc<dyn TextChunker>,
}

impl VaultSearchService {
    /// Wire the three pipeline components.
    #[must_use]
    pub fn new(
        embedding: Arc<dyn EmbeddingPort>,
        repository: Arc<dyn NoteRepository>,
        chunker: Arc<dyn TextChunker>,
    ) -> Self {
        Self {
            embedding,
            repository,
            chunker,
        }
    }

    /// Search the vault index for notes matching the query.
    ///
    /// # Pipeline
    ///
    /// 1. Embed the query text via [`EmbeddingPort`]
    /// 2. Load all indexed chunk vectors via [`NoteRepository`]
    /// 3. Rank by cosine similarity (SIMD-friendly, inline implementation)
    /// 4. Return top-`limit` results sorted by descending score
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError`] if embedding or database access fails.
    #[instrument(skip(self), fields(query_len = query.len(), limit))]
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<VaultSearchResult>, ScraperError> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }

        // Step 1: Embed the query.
        let query_embedding = self
            .embedding
            .embed(query)
            .await
            .map_err(ScraperError::Semantic)?;

        // Step 2: Load all indexed vectors.
        let chunks = self.repository.load_all_vectors().await?;

        if chunks.is_empty() {
            debug!("vault index is empty — no results");
            return Ok(Vec::new());
        }

        // Step 3: Rank by cosine similarity.
        let mut scored: Vec<VaultSearchResult> = chunks
            .into_iter()
            .map(|chunk| {
                let score = cosine_similarity(&query_embedding, &chunk.embedding);
                VaultSearchResult {
                    note_path: chunk.note_path,
                    content: chunk.content,
                    score,
                    chunk_index: chunk.chunk_index,
                    heading: None, // TODO: extract from chunk metadata when NoteChunkVector carries it
                }
            })
            .collect();

        // Sort descending by score.
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        // Step 4: Truncate to limit.
        scored.truncate(limit);

        info!(
            results = scored.len(),
            top_score = scored.first().map_or(0.0, |r| r.score),
            "vault search completed"
        );

        Ok(scored)
    }

    /// Index a single note: chunk → embed → persist.
    ///
    /// The caller is responsible for reading the file and providing its
    /// content. This method handles chunking, embedding, and storage.
    ///
    /// # Pipeline
    ///
    /// 1. Chunk the markdown content via [`TextChunker`]
    /// 2. Embed all chunks via [`EmbeddingPort::embed_batch`]
    /// 3. Register the note via [`NoteRepository::save_note`]
    /// 4. Persist each chunk with its embedding
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError`] if chunking, embedding, or persistence fails.
    #[instrument(skip(self, content), fields(path, content_len = content.len()))]
    pub async fn index_note(
        &self,
        path: &str,
        content: &str,
        content_hash: &str,
        mtime_secs: i64,
    ) -> Result<usize, ScraperError> {
        // Step 1: Chunk the markdown.
        let chunk_texts = self
            .chunker
            .chunk_text(content)
            .map_err(ScraperError::Semantic)?;

        if chunk_texts.is_empty() {
            debug!(path, "no chunks produced — skipping note");
            return Ok(0);
        }

        // Step 2: Embed all chunks in a batch.
        let embeddings = self
            .embedding
            .embed_batch(&chunk_texts)
            .await
            .map_err(ScraperError::Semantic)?;

        // Step 3: Register the note.
        let note_id = self
            .repository
            .save_note(path, content_hash, mtime_secs)
            .await?;

        // Step 4: Persist each chunk with its embedding.
        for (i, (text, emb)) in chunk_texts.iter().zip(embeddings.iter()).enumerate() {
            self.repository
                .save_note_chunk(note_id, i as i64, text, Some(emb))
                .await?;
        }

        debug!(path, chunks = chunk_texts.len(), "note indexed");
        Ok(chunk_texts.len())
    }

    /// Check whether a note needs re-indexing.
    ///
    /// Delegates to [`NoteRepository::note_needs_reindex`] which compares
    /// the stored content hash against the provided one.
    pub async fn needs_reindex(
        &self,
        path: &str,
        content_hash: &str,
    ) -> Result<bool, ScraperError> {
        self.repository.note_needs_reindex(path, content_hash).await
    }

    /// Remove a note and all its chunks from the index.
    pub async fn remove_note(&self, path: &str) -> Result<(), ScraperError> {
        self.repository.delete_note(path).await
    }
}

/// Cosine similarity between two vectors.
///
/// Returns a value in [-1.0, 1.0]. For normalized embedding vectors
/// (as produced by Granite models), this equals the dot product.
/// Returns 0.0 for empty or zero-magnitude vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
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
        0.0
    } else {
        dot / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let v = vec![1.0, 0.0, 0.0];
        let score = cosine_similarity(&v, &v);
        assert!((score - 1.0).abs() < 1e-6, "identical vectors → 1.0");
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!(score.abs() < 1e-6, "orthogonal vectors → 0.0");
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        let score = cosine_similarity(&a, &b);
        assert!((score + 1.0).abs() < 1e-6, "opposite vectors → -1.0");
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0, "zero vector → 0.0");
    }

    #[test]
    fn test_cosine_similarity_dimension_mismatch() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0, "mismatched dims → 0.0");
    }

    #[test]
    fn test_search_result_debug() {
        let result = VaultSearchResult {
            note_path: "vault/rust.md".to_owned(),
            content: "Rust is great".to_owned(),
            score: 0.95,
            chunk_index: 0,
            heading: Some("Introduction".to_owned()),
        };
        let debug = format!("{result:?}");
        assert!(debug.contains("vault/rust.md"));
        assert!(debug.contains("0.95"));
    }
}
