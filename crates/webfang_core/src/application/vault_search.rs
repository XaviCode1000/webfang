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

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info, instrument, warn};

use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::note_repository::NoteRepository;
use crate::domain::text_chunker::TextChunker;
use crate::error::ScraperError;
use crate::infrastructure::obsidian::read_vault_notes;

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

/// Summary of a vault sync operation.
///
/// Returned by [`VaultSearchService::sync_vault`] after reconciling
/// the filesystem vault against the persistent index.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct SyncSummary {
    /// Notes newly indexed (not previously in the index).
    pub indexed: usize,
    /// Notes re-indexed because their content hash changed.
    pub updated: usize,
    /// Notes removed from the index (deleted from the vault).
    pub deleted: usize,
    /// Notes already indexed with matching content hash (skipped).
    pub unchanged: usize,
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
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

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

        // Step 2: Register the note so sync_vault recognizes it as indexed
        // even when it produces no chunks. Without this, a note that yields 0
        // chunks (below min_chunk_size) is never persisted and gets re-indexed
        // on every sync (issue #577).
        let note_id = self
            .repository
            .save_note(path, content_hash, mtime_secs)
            .await?;

        if chunk_texts.is_empty() {
            debug!(path, "no chunks produced — note registered with 0 chunks");
            return Ok(0);
        }

        // Step 3: Embed all chunks in a batch.
        let embeddings = self
            .embedding
            .embed_batch(&chunk_texts)
            .await
            .map_err(ScraperError::Semantic)?;

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

    /// Synchronize the vault filesystem against the persistent index.
    ///
    /// # Pipeline
    ///
    /// 1. Read all `.md` notes from the vault via [`read_vault_notes`]
    /// 2. Load all indexed note metadata via [`NoteRepository::list_indexed_notes`]
    /// 3. Compare content hashes:
    ///    - **New** note (not indexed) → [`index_note`](Self::index_note)
    ///    - **Changed** note (hash differs) → re-index
    ///    - **Deleted** note (indexed but gone from vault) → [`remove_note`](Self::remove_note)
    ///    - **Unchanged** → skip
    /// 4. Return a [`SyncSummary`] with counts
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError`] if vault reading, database access,
    /// embedding, or persistence fails. Individual note indexing errors
    /// are logged and skipped (best-effort sync).
    #[instrument(skip(self), fields(vault_path = %vault_path.display()))]
    pub async fn sync_vault(&self, vault_path: &Path) -> Result<SyncSummary, ScraperError> {
        let start = Instant::now();

        // Step 1: Read all notes from the vault filesystem.
        let notes = read_vault_notes(vault_path)?;

        // Step 2: Load all indexed note metadata.
        let indexed = self.repository.list_indexed_notes().await?;

        // Build lookup structures.
        let indexed_map: HashMap<&str, &str> = indexed
            .iter()
            .map(|m| (m.path.as_str(), m.content_hash.as_str()))
            .collect();
        let vault_paths: HashSet<&str> = notes.iter().map(|n| n.path.as_str()).collect();

        let mut summary = SyncSummary::default();

        // Step 3a: Index new and changed notes.
        for note in &notes {
            match indexed_map.get(note.path.as_str()) {
                None => {
                    // New note — never indexed.
                    match self
                        .index_note(
                            &note.path,
                            &note.content,
                            &note.content_hash,
                            note.mtime_secs,
                        )
                        .await
                    {
                        Ok(_) => summary.indexed += 1,
                        Err(e) => {
                            warn!(path = %note.path, "failed to index note: {e}");
                        },
                    }
                },
                Some(stored_hash) if *stored_hash != note.content_hash => {
                    // Changed note — content hash differs.
                    match self
                        .index_note(
                            &note.path,
                            &note.content,
                            &note.content_hash,
                            note.mtime_secs,
                        )
                        .await
                    {
                        Ok(_) => summary.updated += 1,
                        Err(e) => {
                            warn!(path = %note.path, "failed to re-index note: {e}");
                        },
                    }
                },
                Some(_) => {
                    // Unchanged — hash matches.
                    summary.unchanged += 1;
                },
            }
        }

        // Step 3b: Delete notes that are no longer in the vault.
        for meta in &indexed {
            if !vault_paths.contains(meta.path.as_str()) {
                match self.remove_note(&meta.path).await {
                    Ok(()) => summary.deleted += 1,
                    Err(e) => {
                        warn!(path = %meta.path, "failed to delete stale note: {e}");
                    },
                }
            }
        }

        let duration = start.elapsed();
        info!(
            indexed = summary.indexed,
            updated = summary.updated,
            deleted = summary.deleted,
            unchanged = summary.unchanged,
            duration_ms = duration.as_millis() as u64,
            "vault sync completed"
        );

        Ok(summary)
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

    #[test]
    fn test_sync_summary_default() {
        let summary = SyncSummary::default();
        assert_eq!(summary.indexed, 0);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.deleted, 0);
        assert_eq!(summary.unchanged, 0);
    }

    // --- Stub ports for sync_vault tests ---

    use crate::domain::note_repository::{IndexedNoteMeta, NoteChunkVector};
    use crate::error::SemanticError;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;

    type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// Stub embedder: returns a fixed 3-dim vector for any input.
    struct StubEmbedding;

    impl EmbeddingPort for StubEmbedding {
        fn embed<'a>(&'a self, _text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
            Box::pin(async { Ok(vec![0.1, 0.2, 0.3]) })
        }

        fn embedding_dim(&self) -> usize {
            3
        }
    }

    /// Stub chunker: splits on newlines, one chunk per line.
    struct StubChunker;

    impl TextChunker for StubChunker {
        fn chunk_text(&self, text: &str) -> Result<Vec<String>, SemanticError> {
            Ok(text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(String::from)
                .collect())
        }
    }

    /// In-memory note repository for testing sync logic.
    #[derive(Default)]
    struct InMemoryNoteRepo {
        notes: Mutex<Vec<IndexedNoteMeta>>,
        chunks: Mutex<Vec<NoteChunkVector>>,
        next_id: Mutex<i64>,
    }

    impl NoteRepository for InMemoryNoteRepo {
        fn save_note<'a>(
            &'a self,
            path: &'a str,
            content_hash: &'a str,
            mtime_secs: i64,
        ) -> BoxFuture<'a, Result<i64, ScraperError>> {
            Box::pin(async move {
                let mut notes = self.notes.lock().unwrap();
                // Update existing or insert new.
                if let Some(existing) = notes.iter_mut().find(|n| n.path == path) {
                    existing.content_hash = content_hash.to_owned();
                    existing.mtime_secs = mtime_secs;
                    return Ok(1);
                }
                let mut id = self.next_id.lock().unwrap();
                *id += 1;
                notes.push(IndexedNoteMeta {
                    path: path.to_owned(),
                    content_hash: content_hash.to_owned(),
                    mtime_secs,
                });
                Ok(*id)
            })
        }

        fn save_note_chunk<'a>(
            &'a self,
            note_id: i64,
            chunk_index: i64,
            content: &'a str,
            embedding: Option<&'a [f32]>,
        ) -> BoxFuture<'a, Result<(), ScraperError>> {
            Box::pin(async move {
                let _ = note_id;
                self.chunks.lock().unwrap().push(NoteChunkVector {
                    note_path: String::new(),
                    content: content.to_owned(),
                    chunk_index,
                    embedding: embedding.unwrap_or_default().to_vec(),
                });
                Ok(())
            })
        }

        fn load_all_vectors(&self) -> BoxFuture<'_, Result<Vec<NoteChunkVector>, ScraperError>> {
            Box::pin(async { Ok(self.chunks.lock().unwrap().clone()) })
        }

        fn note_needs_reindex<'a>(
            &'a self,
            path: &'a str,
            content_hash: &'a str,
        ) -> BoxFuture<'a, Result<bool, ScraperError>> {
            Box::pin(async move {
                let notes = self.notes.lock().unwrap();
                match notes.iter().find(|n| n.path == path) {
                    Some(n) => Ok(n.content_hash != content_hash),
                    None => Ok(true),
                }
            })
        }

        fn delete_note<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), ScraperError>> {
            Box::pin(async move {
                self.notes.lock().unwrap().retain(|n| n.path != path);
                Ok(())
            })
        }

        fn list_indexed_notes(&self) -> BoxFuture<'_, Result<Vec<IndexedNoteMeta>, ScraperError>> {
            Box::pin(async { Ok(self.notes.lock().unwrap().clone()) })
        }
    }

    fn test_service(repo: Arc<InMemoryNoteRepo>) -> VaultSearchService {
        VaultSearchService::new(Arc::new(StubEmbedding), repo, Arc::new(StubChunker))
    }

    /// Create a synthetic vault with `.obsidian/` marker and notes.
    fn create_test_vault(tmp: &Path, notes: &[(&str, &str)]) {
        std::fs::create_dir_all(tmp.join(".obsidian")).unwrap();
        for (name, content) in notes {
            if let Some(parent) = Path::new(name).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(tmp.join(parent)).unwrap();
                }
            }
            std::fs::write(tmp.join(name), content).unwrap();
        }
    }

    #[tokio::test]
    async fn sync_vault_indexes_new_notes() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_vault(
            tmp.path(),
            &[("a.md", "# A\nContent A"), ("b.md", "# B\nContent B")],
        );

        let repo = Arc::new(InMemoryNoteRepo::default());
        let service = test_service(repo.clone());

        let summary = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(summary.indexed, 2);
        assert_eq!(summary.updated, 0);
        assert_eq!(summary.deleted, 0);
        assert_eq!(summary.unchanged, 0);

        // Verify notes are in the repo.
        let indexed = repo.list_indexed_notes().await.unwrap();
        assert_eq!(indexed.len(), 2);
    }

    #[tokio::test]
    async fn sync_vault_skips_unchanged_notes() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_vault(tmp.path(), &[("a.md", "# A\nContent A")]);

        let repo = Arc::new(InMemoryNoteRepo::default());
        let service = test_service(repo.clone());

        // First sync: indexes the note.
        let s1 = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(s1.indexed, 1);

        // Second sync: note unchanged.
        let s2 = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(s2.indexed, 0);
        assert_eq!(s2.updated, 0);
        assert_eq!(s2.unchanged, 1);
    }

    #[tokio::test]
    async fn sync_vault_detects_changed_notes() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_vault(tmp.path(), &[("a.md", "# A\nOriginal")]);

        let repo = Arc::new(InMemoryNoteRepo::default());
        let service = test_service(repo.clone());

        // First sync.
        let s1 = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(s1.indexed, 1);

        // Modify the note.
        std::fs::write(tmp.path().join("a.md"), "# A\nModified content").unwrap();

        // Second sync: detects change.
        let s2 = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(s2.indexed, 0);
        assert_eq!(s2.updated, 1);
        assert_eq!(s2.unchanged, 0);
    }

    #[tokio::test]
    async fn sync_vault_deletes_removed_notes() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_vault(
            tmp.path(),
            &[("a.md", "# A"), ("b.md", "# B"), ("c.md", "# C")],
        );

        let repo = Arc::new(InMemoryNoteRepo::default());
        let service = test_service(repo.clone());

        // First sync: 3 notes.
        let s1 = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(s1.indexed, 3);

        // Delete one note.
        std::fs::remove_file(tmp.path().join("b.md")).unwrap();

        // Second sync: detects deletion.
        let s2 = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(s2.deleted, 1);
        assert_eq!(s2.unchanged, 2);

        // Verify only 2 notes remain.
        let indexed = repo.list_indexed_notes().await.unwrap();
        assert_eq!(indexed.len(), 2);
    }

    #[tokio::test]
    async fn sync_vault_empty_vault() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_vault(tmp.path(), &[]);

        let repo = Arc::new(InMemoryNoteRepo::default());
        let service = test_service(repo);

        let summary = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(summary, SyncSummary::default());
    }

    #[tokio::test]
    async fn sync_vault_mixed_operations() {
        let tmp = tempfile::tempdir().unwrap();
        create_test_vault(
            tmp.path(),
            &[
                ("keep.md", "# Keep"),
                ("change.md", "# Old"),
                ("delete.md", "# Gone"),
            ],
        );

        let repo = Arc::new(InMemoryNoteRepo::default());
        let service = test_service(repo.clone());

        // First sync: 3 notes.
        service.sync_vault(tmp.path()).await.unwrap();

        // Modify one, delete one, add one.
        std::fs::write(tmp.path().join("change.md"), "# New content").unwrap();
        std::fs::remove_file(tmp.path().join("delete.md")).unwrap();
        std::fs::write(tmp.path().join("new.md"), "# Brand new").unwrap();

        let summary = service.sync_vault(tmp.path()).await.unwrap();
        assert_eq!(summary.indexed, 1, "new.md");
        assert_eq!(summary.updated, 1, "change.md");
        assert_eq!(summary.deleted, 1, "delete.md");
        assert_eq!(summary.unchanged, 1, "keep.md");
    }
}
