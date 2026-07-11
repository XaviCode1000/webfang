//! Domain trait for vector persistence (dependency inversion).
//!
//! Defines the persistence contract used by the elastic ingestion pipeline.
//! The infrastructure layer
//! ([`crate::infrastructure::persistence::sqlite::SqliteVectorRepository`])
//! implements this trait; the application layer depends on the trait — not the
//! concrete repo — so SQLite can be swapped or mocked without touching
//! orchestration logic.
//!
//! # Dyn-compatibility (A1 desugar — core-slimming)
//!
//! The four trait methods use manual `async fn` desugaring to
//! `Pin<Box<dyn Future<Output = …> + Send + '_>>` (BoxFuture) instead of native
//! `async fn` in traits. This makes the trait **dyn-compatible** so
//! `Arc<dyn VectorRepository + Send + Sync>` can be used for runtime dispatch
//! (spec S3.4), without adding the `async_trait` crate (frozen decision #1).
//! A blanket impl for `Arc<T>` lets `ElasticIngestion<R: VectorRepository>`
//! accept `Arc<dyn VectorRepository + Send + Sync>` as `R`.

use std::future::Future;
use std::pin::Pin;

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
///
/// The methods are desugared to `Pin<Box<dyn Future<…> + Send + '_>>` so the
/// trait is dyn-compatible (A1, spec S3.4) without the `async_trait` crate.
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
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>>;

    /// Save a chunk, optionally with its embedding vector.
    ///
    /// When `embedding` is `Some`, it is serialized to a little-endian `f32`
    /// BLOB; when `None`, the `embedding_vector` column is stored as SQL `NULL`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure (e.g. a
    /// foreign-key violation if `resource_url` was never saved first).
    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>>;

    /// Check whether a resource with this `content_hash` already exists.
    ///
    /// Returns `Ok(Some(url))` with the existing resource's URL if found, or
    /// `Ok(None)` if no resource has that hash.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Persistence`] on any database failure.
    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>>;

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
    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>>;
}

/// Blanket impl so `Arc<dyn VectorRepository + Send + Sync>` satisfies
/// `R: VectorRepository + Send + Sync` in
/// [`crate::application::elastic_ingestion::ElasticIngestion<R>`] (spec S3.4).
///
/// Delegates each method through the `Arc` deref to the inner repository. This
/// is the bridge that lets the Container store
/// `Option<Arc<ElasticIngestion<Arc<dyn VectorRepository + Send + Sync>>>>`
/// for runtime repo dispatch (SQLite when `persistence` is ON, StreamRepository
/// when OFF).
impl<T: VectorRepository + ?Sized> VectorRepository for std::sync::Arc<T> {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        (**self).save_resource(url, title, content_hash, size_bytes)
    }

    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        (**self).save_chunk(id, resource_url, chunk_index, content, embedding)
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        (**self).resource_exists_by_hash(content_hash)
    }

    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        (**self).get_vector(chunk_id)
    }
}
