//! Domain trait for vector persistence (dependency inversion).
//!
//! Defines the persistence contract used by the elastic ingestion pipeline
//! (PR4+). The infrastructure layer
//! ([`crate::infrastructure::persistence::sqlite::SqliteVectorRepository`])
//! implements this trait; the application layer (PR5 orchestrator) depends on
//! the trait — not the concrete repo — so SQLite can be swapped or mocked
//! without touching orchestration logic.
//!
//! Per frozen design decision #8 this lives in its own module (`repository`,
//! singular), separate from the legacy [`crate::domain::repositories`]
//! `CrawlResultRepository`. Consolidation is tracked as a future cleanup.
//!
//! # Async note (forward-looking caveat)
//!
//! This trait uses native `async fn` in traits (stable since Rust 1.75) instead
//! of the `async_trait` proc-macro — the project does not depend on
//! `async_trait` and adding a dependency requires maintainer approval.
//! Consequently the per-method return futures are **not** `Send` by default and
//! the trait is **not** `dyn`-compatible. PR4 tests await these methods on the
//! owning task (no cross-thread send), which works. If PR5 needs to
//! `tokio::spawn` a task that awaits these methods, or needs `dyn
//! VectorRepository` dispatch, it must either (a) await on the spawning task,
//! (b) add the `async_trait` crate with maintainer approval, or (c) desugar the
//! methods to `Pin<Box<dyn Future + Send>>`. See PR4 apply-progress for the full
//! tradeoff analysis.

// `async_fn_in_trait` (a rustc lint, not clippy) warns that native `async fn` in
// traits yields non-`Send` futures and is not `dyn`-compatible. Both are
// intentional, frozen-decision consequences (decision #1: native async traits,
// no `async_trait` dep without maintainer approval) — documented in the module
// docs above and in PR4 apply-progress. Allow at module scope to cover all four
// trait methods without per-method noise.
#![allow(async_fn_in_trait)]

use crate::error::ScraperError;

/// Domain trait for vector persistence (dependency inversion).
///
/// Implementations store crawl resources (with content-hash deduplication) and
/// semantic chunks whose embeddings are serialized as raw little-endian `f32`
/// BLOBs (frozen design decision #7).
///
/// All database failures surface as [`ScraperError::Persistence`] (frozen
/// decision #4: no separate `StorageError` enum), matching the pattern
/// established by PR1.
pub trait VectorRepository: Send + Sync {
    /// Save a resource with its content hash. Returns the resource URL.
    ///
    /// If a resource with the same `content_hash` already exists, this
    /// short-circuits (dedup, frozen decision #3) and returns the **existing**
    /// URL without inserting a duplicate row — saving the heavier chunk inserts.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    async fn save_resource(
        &self,
        url: &str,
        title: &str,
        content_hash: &str,
        size_bytes: u64,
    ) -> Result<String, ScraperError>;

    /// Save a chunk, optionally with its embedding vector.
    ///
    /// When `embedding` is `Some`, it is serialized to a little-endian `f32`
    /// BLOB; when `None`, the `embedding_vector` column is stored as SQL `NULL`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure (e.g. a
    /// foreign-key violation if `resource_url` was never saved first).
    async fn save_chunk(
        &self,
        id: &str,
        resource_url: &str,
        chunk_index: i64,
        content: &str,
        embedding: Option<&[f32]>,
    ) -> Result<(), ScraperError>;

    /// Check whether a resource with this `content_hash` already exists.
    ///
    /// Returns `Ok(Some(url))` with the existing resource's URL if found, or
    /// `Ok(None)` if no resource has that hash.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    async fn resource_exists_by_hash(
        &self,
        content_hash: &str,
    ) -> Result<Option<String>, ScraperError>;

    /// Get the embedding vector for a chunk.
    ///
    /// Returns `Ok(Some(vec))` if the chunk exists and has an embedding,
    /// `Ok(None)` if the chunk is missing or has a `NULL` embedding, or an
    /// error if the stored BLOB is corrupt (length not a multiple of 4).
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on a corrupt BLOB or database
    /// failure.
    async fn get_vector(&self, chunk_id: &str) -> Result<Option<Vec<f32>>, ScraperError>;
}
