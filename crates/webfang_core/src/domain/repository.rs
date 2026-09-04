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
    #[allow(clippy::type_complexity)]
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

/// Erased vector repository for runtime dispatch.
///
/// Lets the Container hold either the SQLite-backed [`crate::infrastructure::persistence::sqlite::SqliteVectorRepository`]
/// (under the `persistence` feature) or the dependency-free `StreamRepository`
/// JSONL sink behind a single type, enabling `Arc<dyn VectorRepository + Send + Sync>`
/// to satisfy the `R: VectorRepository + Send + Sync` bound on
/// [`crate::application::elastic_ingestion::ElasticIngestion`].
pub type DynVectorRepository = std::sync::Arc<dyn VectorRepository + Send + Sync>;

/// Composite [`VectorRepository`] that fans out every call to a set of inner
/// repositories, so the elastic pipeline can persist to SQLite (`--elastic`)
/// **and** emit JSONL (`--output-vectors`) simultaneously (issue #636).
///
/// Fan-out pattern: every write is attempted on all inner sinks, an
/// individual failure is logged but the remaining sinks still run, and the
/// composite reports success if *at least one* sink succeeded. Read-style
/// lookups (`resource_exists_by_hash`,
/// `get_vector`) return the first `Some` produced by any inner repository.
///
/// This is what turns `--elastic` and `--output-vectors` from mutually
/// exclusive `if/else if` branches into orthogonal data destinations — the
/// single `ElasticIngestion` is built over one of these fan-outs, so no vector
/// output is silently dropped.
pub struct MultiVectorRepository {
    repos: Vec<DynVectorRepository>,
}

impl MultiVectorRepository {
    /// Build a fan-out repository over the given inner sinks.
    ///
    /// An empty set is not an invariant violation here — it simply reports an
    /// error on every write. The
    /// [`Container`](crate::application::container::Container) never builds an
    /// empty fan-out (it returns early when no sink is active).
    pub fn new(repos: Vec<DynVectorRepository>) -> Self {
        Self { repos }
    }
}

impl VectorRepository for MultiVectorRepository {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            if self.repos.is_empty() {
                return Err(ScraperError::persistence(
                    "no hay sinks de vectores registrados",
                ));
            }
            let mut first_url: Option<String> = None;
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo
                    .save_resource(url, title, content_hash, size_bytes)
                    .await
                {
                    Ok(u) => {
                        if first_url.is_none() {
                            first_url = Some(u);
                        }
                    },
                    Err(e) => {
                        tracing::error!(error = %e, "vector sink save_resource failed");
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            match first_url {
                Some(u) => Ok(u),
                None => Err(first_err.unwrap_or_else(|| {
                    ScraperError::persistence(
                        "todos los sinks de vectores fallaron al guardar el recurso",
                    )
                })),
            }
        })
    }

    fn save_chunk<'a>(
        &'a self,
        id: &'a str,
        resource_url: &'a str,
        chunk_index: i64,
        content: &'a str,
        embedding: Option<&'a [f32]>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            if self.repos.is_empty() {
                return Err(ScraperError::persistence(
                    "no hay sinks de vectores registrados",
                ));
            }
            let mut any_ok = false;
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo
                    .save_chunk(id, resource_url, chunk_index, content, embedding)
                    .await
                {
                    Ok(()) => any_ok = true,
                    Err(e) => {
                        tracing::error!(error = %e, "vector sink save_chunk failed");
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            if any_ok {
                Ok(())
            } else {
                Err(first_err.unwrap_or_else(|| {
                    ScraperError::persistence(
                        "todos los sinks de vectores fallaron al guardar el chunk",
                    )
                }))
            }
        })
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo.resource_exists_by_hash(content_hash).await {
                    Ok(Some(url)) => return Ok(Some(url)),
                    Ok(None) => {},
                    Err(e) => {
                        tracing::error!(error = %e, "vector sink resource_exists_by_hash failed");
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(None),
            }
        })
    }

    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let mut first_err: Option<ScraperError> = None;
            for repo in &self.repos {
                match repo.get_vector(chunk_id).await {
                    Ok(Some(v)) => return Ok(Some(v)),
                    Ok(None) => {},
                    Err(e) => {
                        tracing::error!(error = %e, "vector sink get_vector failed");
                        if first_err.is_none() {
                            first_err = Some(e);
                        }
                    },
                }
            }
            match first_err {
                Some(e) => Err(e),
                None => Ok(None),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Minimal in-memory sink for asserting fan-out behavior and failures.
    struct InMemoryRepo {
        saves: Arc<AtomicUsize>,
        chunks: Arc<AtomicUsize>,
        fail_write: bool,
    }

    impl InMemoryRepo {
        fn new(saves: Arc<AtomicUsize>, chunks: Arc<AtomicUsize>, fail_write: bool) -> Self {
            Self {
                saves,
                chunks,
                fail_write,
            }
        }
    }

    impl VectorRepository for InMemoryRepo {
        fn save_resource<'a>(
            &'a self,
            url: &'a str,
            _title: &'a str,
            _content_hash: &'a str,
            _size_bytes: u64,
        ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
            Box::pin(async move {
                self.saves.fetch_add(1, Ordering::SeqCst);
                if self.fail_write {
                    Err(ScraperError::persistence("mock save_resource failure"))
                } else {
                    Ok(url.to_string())
                }
            })
        }

        fn save_chunk<'a>(
            &'a self,
            _id: &'a str,
            _resource_url: &'a str,
            _chunk_index: i64,
            _content: &'a str,
            _embedding: Option<&'a [f32]>,
        ) -> Pin<Box<dyn Future<Output = Result<(), ScraperError>> + Send + 'a>> {
            Box::pin(async move {
                self.chunks.fetch_add(1, Ordering::SeqCst);
                if self.fail_write {
                    Err(ScraperError::persistence("mock save_chunk failure"))
                } else {
                    Ok(())
                }
            })
        }

        fn resource_exists_by_hash<'a>(
            &'a self,
            _content_hash: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>>
        {
            Box::pin(async { Ok(None) })
        }

        fn get_vector<'a>(
            &'a self,
            _chunk_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>>
        {
            Box::pin(async { Ok(None) })
        }
    }

    fn repo(
        saves: &Arc<AtomicUsize>,
        chunks: &Arc<AtomicUsize>,
        fail_write: bool,
    ) -> DynVectorRepository {
        Arc::new(InMemoryRepo::new(saves.clone(), chunks.clone(), fail_write))
    }

    #[tokio::test]
    async fn fan_out_writes_to_all_inner_sinks() {
        let s1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let multi = MultiVectorRepository::new(vec![repo(&s1, &c1, false), repo(&s2, &c2, false)]);

        multi.save_resource("u", "t", "hash", 1).await.unwrap();
        multi
            .save_chunk("id", "u", 0, "c", Some(&[0.5]))
            .await
            .unwrap();

        assert_eq!(s1.load(Ordering::SeqCst), 1);
        assert_eq!(s2.load(Ordering::SeqCst), 1);
        assert_eq!(c1.load(Ordering::SeqCst), 1);
        assert_eq!(c2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn continues_on_single_sink_failure() {
        let s1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let multi = MultiVectorRepository::new(vec![
            repo(&s1, &c1, true),  // fails
            repo(&s2, &c2, false), // ok
        ]);

        assert!(multi
            .save_chunk("id", "u", 0, "c", Some(&[0.5]))
            .await
            .is_ok());
        assert!(multi.save_resource("u", "t", "h", 1).await.is_ok());
        // Best-effort: the failing sink was still attempted.
        assert_eq!(s1.load(Ordering::SeqCst), 1);
        assert_eq!(s2.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn all_sinks_failing_returns_error() {
        let s1 = Arc::new(AtomicUsize::new(0));
        let c1 = Arc::new(AtomicUsize::new(0));
        let s2 = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::new(AtomicUsize::new(0));

        let multi = MultiVectorRepository::new(vec![repo(&s1, &c1, true), repo(&s2, &c2, true)]);

        let err = multi
            .save_chunk("id", "u", 0, "c", Some(&[0.5]))
            .await
            .unwrap_err();
        assert!(matches!(err, ScraperError::Persistence(_)));
    }

    #[tokio::test]
    async fn empty_fanout_reports_error() {
        let multi = MultiVectorRepository::new(vec![]);
        let err = multi.save_resource("u", "t", "h", 1).await.unwrap_err();
        assert!(matches!(err, ScraperError::Persistence(_)));
    }
}
