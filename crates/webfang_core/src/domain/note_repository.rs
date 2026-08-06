//! Domain trait for vault note persistence (dependency inversion).
//!
//! Defines the persistence contract for Obsidian vault note chunks and their
//! embedding vectors. Separate from [`super::repository::VectorRepository`]
//! (ISP): the crawl-oriented repo uses `url`/`content_hash` semantics that
//! don't apply to local filesystem notes.
//!
//! The infrastructure layer implements this trait with SQLite storage;
//! the application layer ([`crate::application::vault_search`]) depends on
//! the trait — not the concrete repo — so SQLite can be swapped or mocked.
//!
//! # Dyn-compatibility
//!
//! Methods use manual `async fn` desugaring to
//! `Pin<Box<dyn Future<Output = …> + Send + '_>>` (BoxFuture) instead of
//! native `async fn` in traits, matching the pattern in
//! [`super::repository::VectorRepository`] (frozen decision #1: no
//! `async_trait` crate).

use std::future::Future;
use std::pin::Pin;

use crate::error::ScraperError;

/// A boxed future for dyn-compatible async traits.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A stored note chunk with its embedding vector.
///
/// Returned by bulk vector queries for in-memory ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteChunkVector {
    /// Filesystem path of the source note (e.g. `vault/notes/rust.md`).
    pub note_path: String,
    /// The chunk's text content.
    pub content: String,
    /// Zero-based chunk index within the note.
    pub chunk_index: i64,
    /// The embedding vector (384d for Granite models).
    pub embedding: Vec<f32>,
}

/// Metadata about an indexed note for staleness detection.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedNoteMeta {
    /// Filesystem path of the note.
    pub path: String,
    /// SHA-256 content hash (hex, lowercase).
    pub content_hash: String,
    /// Last modification time (Unix epoch seconds).
    pub mtime_secs: i64,
}

/// Domain trait for vault note vector persistence.
///
/// Implementations store note metadata (path, content hash, mtime) and
/// their chunked embedding vectors. Embeddings are serialized as raw
/// little-endian `f32` BLOBs, matching the frozen design decision #7
/// used by [`super::repository::VectorRepository`].
///
/// All database failures surface as [`ScraperError::Persistence`],
/// matching the error stratification pattern (frozen decision #4).
pub trait NoteRepository: Send + Sync {
    /// Register or update a note's metadata. Returns the note's row ID.
    ///
    /// If a note with the same `path` already exists, updates its
    /// `content_hash` and `mtime_secs` and returns the existing ID.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    fn save_note<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
        mtime_secs: i64,
    ) -> BoxFuture<'a, Result<i64, ScraperError>>;

    /// Save a chunk with its embedding for a previously saved note.
    ///
    /// The `note_id` must reference an existing note (from [`save_note`](Self::save_note)).
    /// Embeddings are serialized as little-endian `f32` BLOBs.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on database failure (e.g.
    /// foreign-key violation if `note_id` was never saved).
    fn save_note_chunk<'a>(
        &'a self,
        note_id: i64,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
    ) -> BoxFuture<'a, Result<(), ScraperError>>;

    /// Atomically register a note and persist all its chunks in one database
    /// transaction.
    ///
    /// This replaces the separate [`save_note`](Self::save_note) +
    /// [`save_note_chunk`](Self::save_note_chunk) calls when the caller has
    /// all chunk data up front. A transaction guarantees that a note is never
    /// persisted without its chunks — if the process panics or the embedding
    /// batch fails after `save_note`, the whole operation rolls back instead
    /// of leaving an unsearchable "ghost" note (#577 follow-up: atomicity).
    ///
    /// `chunks` is `[(text, embedding)]`. An empty `chunks` slice still
    /// registers the note (so `sync_vault` won't re-index it) but writes no
    /// chunk rows.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure. The
    /// transaction is rolled back automatically on error.
    fn index_note_transactional<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
        mtime_secs: i64,
        chunks: &'a [(&'a str, Option<&'a [f32]>)],
    ) -> BoxFuture<'a, Result<i64, ScraperError>>;

    /// Load all chunk vectors for in-memory ranking.
    ///
    /// Returns every indexed chunk with its embedding. Chunks without
    /// embeddings (`NULL` in SQLite) are excluded.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on database failure or
    /// corrupt embedding BLOBs.
    fn load_all_vectors(&self) -> BoxFuture<'_, Result<Vec<NoteChunkVector>, ScraperError>>;

    /// Check whether a note needs re-indexing.
    ///
    /// Returns `true` if the note is not indexed, or if the stored
    /// `content_hash` differs from the provided one (content changed).
    /// Returns `false` if the note is indexed and the hash matches.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on database failure.
    fn note_needs_reindex<'a>(
        &'a self,
        path: &'a str,
        content_hash: &'a str,
    ) -> BoxFuture<'a, Result<bool, ScraperError>>;

    /// Delete a note and all its chunks.
    ///
    /// Used when a note is removed from the vault. Cascades to delete
    /// all associated chunks.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on database failure.
    fn delete_note<'a>(&'a self, path: &'a str) -> BoxFuture<'a, Result<(), ScraperError>>;

    /// List metadata for all indexed notes.
    ///
    /// Used for staleness detection: compare stored hashes/mtimes
    /// against the filesystem to find notes that need re-indexing.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on database failure.
    fn list_indexed_notes(&self) -> BoxFuture<'_, Result<Vec<IndexedNoteMeta>, ScraperError>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_chunk_vector_debug() {
        let chunk = NoteChunkVector {
            note_path: "vault/rust.md".to_owned(),
            content: "Rust is great".to_owned(),
            chunk_index: 0,
            embedding: vec![0.1, 0.2, 0.3],
        };
        let debug = format!("{chunk:?}");
        assert!(debug.contains("vault/rust.md"));
        assert!(debug.contains("Rust is great"));
    }

    #[test]
    fn test_indexed_note_meta_clone_eq() {
        let meta = IndexedNoteMeta {
            path: "vault/note.md".to_owned(),
            content_hash: "abc123".to_owned(),
            mtime_secs: 1_700_000_000,
        };
        let cloned = meta.clone();
        assert_eq!(meta, cloned);
    }

    #[test]
    fn test_note_repository_is_object_safe() {
        fn assert_dyn_compatible(_: &dyn NoteRepository) {}
        // Compile-time check: if this compiles, the trait is object-safe.
        // We can't call assert_dyn_compatible without an impl, but the
        // function definition itself proves object safety.
        let _ = assert_dyn_compatible as fn(&dyn NoteRepository);
    }
}
