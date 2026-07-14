//! SQLite schema, WAL pragmas, and connection pool for elastic ingestion.
//!
//! Schema is frozen per `design.md` (the authoritative frozen artifact). The
//! task-list DDL transcription differed in three places; this implementation
//! follows the frozen design:
//! - `chunks.id` is `TEXT` (required by `VectorRepository::save_vector(id: &str)`
//!   in PR4 — the task's `INTEGER` would force a destructive migration).
//! - `created_at` is `TEXT` (SQLite stores timestamps as TEXT regardless; the
//!   task's `TIMESTAMP` is cosmetic but diverges from design).
//! - the `content_hash` index lives on `resources` (which has the column); the
//!   task's `idx_chunks_hash ON chunks(content_hash)` referenced a non-existent
//!   column and would fail `setup_schema`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use deadpool_sqlite::{Config, Hook, HookError, Manager, Pool, Runtime};

use crate::domain::repository::VectorRepository;
use crate::error::ScraperError;

// ============================================================================
// Schema (frozen design.md §schema)
// ============================================================================

/// WAL + per-connection pragmas, applied on every new pooled connection via the
/// `post_create` hook. `journal_mode=WAL` is persistent (DB header); the other
/// two are per-connection and therefore MUST be set on each connection.
const INIT_PRAGMAS: &str = "\
PRAGMA journal_mode = WAL;\
PRAGMA synchronous = NORMAL;\
PRAGMA cache_size = -4000;";

/// Forward-only schema (`CREATE ... IF NOT EXISTS`). Run once by
/// [`SqliteVectorRepository::setup_schema`]. No destructive migration in v1.
const SCHEMA_DDL: &str = "\
CREATE TABLE IF NOT EXISTS resources (\
    url TEXT PRIMARY KEY,\
    title TEXT,\
    content_hash TEXT,\
    size_bytes INTEGER,\
    status TEXT,\
    created_at TEXT,\
    updated_at TEXT,\
    metadata_json TEXT\
);\
CREATE TABLE IF NOT EXISTS chunks (\
    id TEXT PRIMARY KEY,\
    resource_url TEXT NOT NULL REFERENCES resources(url),\
    chunk_index INTEGER,\
    content TEXT,\
    embedding_vector BLOB,\
    created_at TEXT\
);\
CREATE INDEX IF NOT EXISTS idx_chunks_resource ON chunks(resource_url);\
CREATE INDEX IF NOT EXISTS idx_resources_content_hash ON resources(content_hash);";

// ============================================================================
// Pool construction
// ============================================================================

/// Build a WAL-mode SQLite connection pool backed by [`deadpool_sqlite`].
///
/// `pool_size` is clamped to at least 1 (the caller — `ElasticConfig` — already
/// applies the frozen `cpu_cores`/floor-4 sizing). The parent directory of
/// `db_path` is created if missing. Per-connection pragmas are applied via a
/// `post_create` hook so every connection honours the WAL-mode contract.
///
/// The pool is created lazily — the database file is only materialized on the
/// first [`Pool::get`] (i.e. inside [`SqliteVectorRepository::setup_schema`]),
/// which is the explicit fail-fast point if the path is not writable.
pub fn create_pool(db_path: &Path, pool_size: usize) -> Result<Pool, ScraperError> {
    // Fail-fast: ensure the parent directory exists before opening the DB.
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ScraperError::persistence(format!("crear directorio {parent:?}: {e}"))
            })?;
        }
    }

    let cfg = Config::new(db_path.to_path_buf());
    let manager = Manager::from_config(&cfg, Runtime::Tokio1);
    let pool = Pool::builder(manager)
        .max_size(pool_size.max(1))
        .runtime(Runtime::Tokio1)
        .post_create(pragma_hook())
        .build()
        .map_err(|e| ScraperError::persistence(format!("construir pool SQLite: {e}")))?;
    Ok(pool)
}

/// Create a SQLite pool backed by `:memory:` for testing.
///
/// Uses a single connection with no WAL pragmas (in-memory databases don't
/// support WAL mode). The pool **must** be kept alive for the entire test
/// lifetime — dropping the pool closes all connections and destroys the
/// in-memory database.
pub fn create_memory_pool() -> Result<Pool, ScraperError> {
    let cfg = Config::new(PathBuf::from(":memory:"));
    let manager = Manager::from_config(&cfg, Runtime::Tokio1);
    let pool = Pool::builder(manager)
        .max_size(1)
        .runtime(Runtime::Tokio1)
        // No post_create hook: in-memory databases don't support WAL mode.
        .build()
        .map_err(|e| ScraperError::persistence(format!("construir pool SQLite en memoria: {e}")))?;
    Ok(pool)
}

/// `post_create` hook applying the WAL-mode pragmas to each new connection.
fn pragma_hook() -> Hook {
    Hook::async_fn(|obj, _metrics| {
        Box::pin(async move {
            obj.interact(|conn| conn.execute_batch(INIT_PRAGMAS))
                .await
                .map_err(|e| HookError::message(format!("init pragmas (interact): {e}")))?
                .map_err(|e| HookError::message(format!("init pragmas: {e}")))?;
            Ok(())
        })
    })
}

// ============================================================================
// SqliteVectorRepository (PR1: schema init; PR4 adds CRUD + dedup)
// ============================================================================

/// SQLite-backed vector repository for the elastic ingestion pipeline.
///
/// PR1 provides explicit schema initialization only; the `VectorRepository`
/// trait CRUD (`save_vector`, `get_vector`) and content-hash deduplication land
/// in PR4. Per frozen design decision #1, schema creation is **explicit** —
/// [`SqliteVectorRepository::setup_schema`] MUST be called at startup. There is
/// no implicit schema creation on first connection.
#[derive(Debug, Clone)]
pub struct SqliteVectorRepository {
    pool: Pool,
}

impl SqliteVectorRepository {
    /// Wrap an existing pool. The pool should already have had
    /// [`setup_schema`] run against it.
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Create a repository backed by an in-memory SQLite database.
    ///
    /// Intended for integration tests — no disk I/O, automatic cleanup on drop.
    /// Does **not** call [`setup_schema`]; the caller must do that explicitly
    /// if tables are needed.
    pub fn from_memory() -> Result<Self, ScraperError> {
        let pool = create_memory_pool()?;
        Ok(Self::new(pool))
    }

    /// Borrow the underlying pool (used by PR2+ wiring and tests).
    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Explicitly initialize the database schema (tables + indexes).
    ///
    /// **MUST** be called once at startup (frozen design decision #1). Acquires
    /// a pooled connection — which also materializes the database file and runs
    /// the per-connection pragmas via the `post_create` hook — making this the
    /// fail-fast point if the configured `db_path` is not writable.
    ///
    /// The DDL is idempotent (`CREATE ... IF NOT EXISTS`), so calling it on an
    /// already-initialized database is a safe no-op.
    pub async fn setup_schema(pool: &Pool) -> Result<(), ScraperError> {
        let conn = pool
            .get()
            .await
            .map_err(|e| ScraperError::persistence(format!("obtener conexión del pool: {e}")))?;
        conn.interact(|c| c.execute_batch(SCHEMA_DDL))
            .await
            .map_err(|e| ScraperError::persistence(format!("ddl (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("ddl schema: {e}")))?;
        Ok(())
    }
}

// ============================================================================
// Embedding BLOB serialization (frozen design decision #7)
// ============================================================================

/// Serialize an `f32` slice to little-endian bytes (4 bytes per `f32`).
///
/// Frozen design decision #7: `embedding_vector` BLOB = raw little-endian `f32`
/// bytes. Uses explicit `f32::to_le_bytes` (no `unsafe`, no `bytemuck`
/// dependency) for portability across architectures.
fn f32_slice_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for &f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// Deserialize little-endian bytes back to `Vec<f32>`.
///
/// Validates the BLOB length is a multiple of 4; returns a
/// [`ScraperError::Persistence`] with a Spanish message on corruption (frozen
/// decision #4: no separate `StorageError` enum — maps to the existing
/// Display-based `Persistence(String)` variant).
fn bytes_to_f32_vec(b: &[u8]) -> Result<Vec<f32>, ScraperError> {
    if !b.len().is_multiple_of(4) {
        return Err(ScraperError::persistence(format!(
            "vector BLOB corrupto: longitud {} no es múltiplo de 4",
            b.len()
        )));
    }
    b.chunks_exact(4)
        .map(|chunk| {
            // `chunks_exact(4)` yields exactly 4-byte slices, so `try_into` is
            // infallible here. A failure would indicate a stdlib bug — hence
            // `expect` for a true invariant (rust-skills `err-expect-bugs-only`).
            let arr: [u8; 4] = chunk
                .try_into()
                .expect("chunks_exact(4) garantiza 4 bytes por fragmento");
            Ok(f32::from_le_bytes(arr))
        })
        .collect()
}

// ============================================================================
// VectorRepository impl (PR4): CRUD + content-hash dedup
// ============================================================================

impl VectorRepository for SqliteVectorRepository {
    fn save_resource<'a>(
        &'a self,
        url: &'a str,
        title: &'a str,
        content_hash: &'a str,
        size_bytes: u64,
    ) -> Pin<Box<dyn Future<Output = Result<String, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            // Dedup short-circuit (frozen decision #3): if the content_hash already
            // exists, return the existing URL and skip the INSERT (I/O saved).
            if let Some(existing) = self.resource_exists_by_hash(content_hash).await? {
                tracing::debug!(
                    content_hash,
                    existing_url = %existing,
                    "dedup: recurso ya persistido, omitiendo inserción"
                );
                return Ok(existing);
            }

            // Own all borrowed inputs: the `interact` closure must be `Send + 'static`
            // and cannot borrow `&str` from this function's frame.
            let url_owned = url.to_string();
            let title_owned = title.to_string();
            let hash_owned = content_hash.to_string();
            let url_for_row = url_owned.clone();

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                c.execute(
                    "INSERT INTO resources \
                     (url, title, content_hash, size_bytes, status, created_at, updated_at, metadata_json) \
                     VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), datetime('now'), NULL)",
                    rusqlite::params![url_for_row, title_owned, hash_owned, size_bytes as i64, "active"],
                )
            })
            .await
            .map_err(|e| ScraperError::persistence(format!("save_resource (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("save_resource: {e}")))?;
            Ok(url_owned)
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
            // Pre-serialize the embedding to owned bytes: the `interact` closure must
            // be `Send + 'static`, so it cannot borrow `&[f32]` from this frame.
            let blob: Option<Vec<u8>> = embedding.map(f32_slice_to_bytes);
            let id = id.to_string();
            let resource_url = resource_url.to_string();
            let content = content.to_string();

            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            conn.interact(move |c| {
                c.execute(
                    "INSERT INTO chunks \
                     (id, resource_url, chunk_index, content, embedding_vector, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                    rusqlite::params![id, resource_url, chunk_index, content, blob.as_deref()],
                )
            })
            .await
            .map_err(|e| ScraperError::persistence(format!("save_chunk (interact): {e}")))?
            .map_err(|e| ScraperError::persistence(format!("save_chunk: {e}")))?;
            Ok(())
        })
    }

    fn resource_exists_by_hash<'a>(
        &'a self,
        content_hash: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let hash = content_hash.to_string();
            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let row: rusqlite::Result<String> = conn
                .interact(move |c| {
                    c.query_row(
                        "SELECT url FROM resources WHERE content_hash = ?1 LIMIT 1",
                        rusqlite::params![hash],
                        |row| row.get::<_, String>(0),
                    )
                })
                .await
                .map_err(|e| {
                    ScraperError::persistence(format!("resource_exists_by_hash (interact): {e}"))
                })?;
            match row {
                Ok(url) => Ok(Some(url)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(ScraperError::persistence(format!(
                    "resource_exists_by_hash: {e}"
                ))),
            }
        })
    }

    fn get_vector<'a>(
        &'a self,
        chunk_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<Vec<f32>>, ScraperError>> + Send + 'a>> {
        Box::pin(async move {
            let id = chunk_id.to_string();
            let conn = self.pool.get().await.map_err(|e| {
                ScraperError::persistence(format!("obtener conexión del pool: {e}"))
            })?;
            let row: rusqlite::Result<Option<Vec<u8>>> = conn
                .interact(move |c| {
                    c.query_row(
                        "SELECT embedding_vector FROM chunks WHERE id = ?1 LIMIT 1",
                        rusqlite::params![id],
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )
                })
                .await
                .map_err(|e| ScraperError::persistence(format!("get_vector (interact): {e}")))?;
            match row {
                Ok(Some(bytes)) => Ok(Some(bytes_to_f32_vec(&bytes)?)),
                Ok(None) => Ok(None),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(ScraperError::persistence(format!("get_vector: {e}"))),
            }
        })
    }
}

/// Free-function alias for [`SqliteVectorRepository::setup_schema`] so callers
/// that only hold a `&Pool` (without constructing the repository) can init the
/// schema at startup.
pub async fn setup_schema(pool: &Pool) -> Result<(), ScraperError> {
    SqliteVectorRepository::setup_schema(pool).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ====================================================================
    // Pure serialization round-trips (Miri-safe — no FFI, no I/O)
    // ====================================================================

    #[test]
    fn test_embedding_roundtrip() {
        // Triangulation: non-trivial vectors incl. empty, negatives, zero, and a
        // 384-dim all-MiniLM-L6-v2-shaped vector. Exact equality must hold.
        for v in [
            vec![],
            vec![0.0_f32],
            vec![1.5_f32, -2.25, 3.0, 0.0, -0.0, f32::MIN_POSITIVE],
            vec![-1.0_f32; 384],
        ] {
            let bytes = f32_slice_to_bytes(&v);
            assert_eq!(bytes.len(), v.len() * 4, "byte length must be 4×f32 count");
            let back = bytes_to_f32_vec(&bytes).expect("roundtrip must succeed");
            assert_eq!(back, v, "f32 vector must round-trip exactly");
        }
    }

    #[test]
    fn test_bytes_to_f32_vec_rejects_invalid_blob_length() {
        // 5 bytes is not a multiple of 4 → corrupt BLOB → Persistence error.
        let bad = [1_u8, 2, 3, 4, 5];
        let err = bytes_to_f32_vec(&bad).expect_err("odd-length BLOB must error");
        let msg = err.to_string();
        assert!(msg.contains("corrupto"), "missing Spanish marker: {msg}");
        assert!(msg.contains('5'), "missing length in message: {msg}");
    }

    // ====================================================================
    // SQLite-backed integration tests
    //
    // Miri es un intérprete de MIR, no un emulador de binarios. No puede
    // ejecutar código C nativo (libsqlite3-sys → sqlite3_threadsafe FFI).
    // Estos tests requieren SQLite3 real del sistema operativo.
    //
    // Estrategia: Aislamiento de Capas (Layer Isolation) — el bloque entero
    // se excluye de Miri con #[cfg(not(miri))] en lugar de parchear cada
    // test uno por uno. Misma práctica que tokio, serde, regex.
    // ====================================================================

    #[cfg(not(miri))]
    mod sqlite {
        use super::*;

        /// Build a fresh pool over a temp-file DB and run the schema.
        async fn fresh_pool(size: usize) -> (TempDir, Pool) {
            let dir = TempDir::new().expect("tempdir");
            let db_path = dir.path().join("crawl.db");
            let pool = create_pool(&db_path, size).expect("create_pool");
            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("setup_schema");
            (dir, pool)
        }

        /// Count rows in `sqlite_master` matching `type` and `name`.
        async fn master_count(pool: &Pool, kind: &str, name: &str) -> i64 {
            // Own the strings: the `interact` closure must be `Send + 'static`, so it
            // cannot capture borrowed `&str` from this function's frame.
            let kind = kind.to_string();
            let name = name.to_string();
            let conn = pool.get().await.expect("get");
            conn.interact(move |c| {
                c.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = ?1 AND name = ?2",
                    rusqlite::params![kind, name],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .expect("interact")
            .expect("count")
        }

        async fn pragma_string(pool: &Pool, pragma: &str) -> String {
            let sql = format!("PRAGMA {pragma}");
            let conn = pool.get().await.expect("get");
            conn.interact(move |c| c.query_row(&sql, [], |row| row.get::<_, String>(0)))
                .await
                .expect("interact")
                .expect("pragma row")
        }

        async fn pragma_int(pool: &Pool, pragma: &str) -> i64 {
            let sql = format!("PRAGMA {pragma}");
            let conn = pool.get().await.expect("get");
            conn.interact(move |c| c.query_row(&sql, [], |row| row.get::<_, i64>(0)))
                .await
                .expect("interact")
                .expect("pragma row")
        }

        #[tokio::test]
        async fn test_setup_schema_creates_resources_table() {
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(master_count(&pool, "table", "resources").await, 1);
        }

        #[tokio::test]
        async fn test_setup_schema_creates_chunks_table() {
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(master_count(&pool, "table", "chunks").await, 1);
        }

        #[tokio::test]
        async fn test_setup_schema_creates_indexes() {
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(
                master_count(&pool, "index", "idx_chunks_resource").await,
                1,
                "idx_chunks_resource missing"
            );
            assert_eq!(
                master_count(&pool, "index", "idx_resources_content_hash").await,
                1,
                "idx_resources_content_hash missing"
            );
        }

        #[tokio::test]
        async fn test_wal_pragmas_applied_to_connection() {
            // A freshly-created pooled connection must carry all three pragmas
            // (proves the post_create hook runs on each connection).
            let (_dir, pool) = fresh_pool(4).await;
            assert_eq!(pragma_string(&pool, "journal_mode").await, "wal");
            // synchronous: 0=OFF,1=NORMAL,2=FULL,3=EXTRA — NORMAL must be 1.
            assert_eq!(pragma_int(&pool, "synchronous").await, 1);
            assert_eq!(pragma_int(&pool, "cache_size").await, -4000);
        }

        #[tokio::test]
        async fn test_db_file_created_when_absent() {
            let dir = TempDir::new().expect("tempdir");
            let db_path = dir.path().join("nested").join("crawl.db");
            assert!(!db_path.exists());
            let pool = create_pool(&db_path, 4).expect("create_pool");
            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("setup_schema");
            // setup_schema materializes the DB file (parent dir created by create_pool).
            assert!(db_path.exists(), "DB file should exist after setup_schema");
        }

        #[tokio::test]
        async fn test_pool_size_is_configurable() {
            let (_dir, pool) = fresh_pool(8).await;
            let status = pool.status();
            assert_eq!(status.max_size, 8, "pool max_size must honor configuration");
        }

        #[tokio::test]
        async fn test_setup_schema_is_idempotent() {
            let (_dir, pool) = fresh_pool(4).await;
            // Running setup_schema twice must not error (CREATE ... IF NOT EXISTS).
            SqliteVectorRepository::setup_schema(&pool)
                .await
                .expect("second setup_schema should be a no-op");
            assert_eq!(master_count(&pool, "table", "resources").await, 1);
            assert_eq!(master_count(&pool, "table", "chunks").await, 1);
        }

        #[tokio::test]
        async fn test_repository_wraps_pool() {
            let (_dir, pool) = fresh_pool(4).await;
            let repo = SqliteVectorRepository::new(pool.clone());
            assert_eq!(repo.pool().status().max_size, 4);
        }

        // ================================================================
        // PR4: VectorRepository CRUD + content-hash dedup (Strict TDD)
        // ================================================================

        /// Build a fresh schema-initialized repo over a temp-file DB.
        async fn fresh_repo(size: usize) -> (TempDir, SqliteVectorRepository) {
            let (dir, pool) = fresh_pool(size).await;
            let repo = SqliteVectorRepository::new(pool);
            (dir, repo)
        }

        /// Count rows in `resources` matching a content_hash (test inspection only).
        async fn count_resources_by_hash(repo: &SqliteVectorRepository, hash: &str) -> i64 {
            let hash = hash.to_string();
            let conn = repo.pool().get().await.expect("get");
            conn.interact(move |c| {
                c.query_row(
                    "SELECT COUNT(*) FROM resources WHERE content_hash = ?1",
                    rusqlite::params![hash],
                    |row| row.get::<_, i64>(0),
                )
            })
            .await
            .expect("interact")
            .expect("count")
        }

        // --- resource_exists_by_hash ---

        #[tokio::test]
        async fn test_resource_exists_by_hash_returns_none_when_empty() {
            let (_dir, repo) = fresh_repo(4).await;
            let got = repo.resource_exists_by_hash("nope").await.expect("query");
            assert!(got.is_none(), "fresh DB has no resources");
        }

        // --- save_resource + dedup (frozen decision #3) ---

        #[tokio::test]
        async fn test_save_and_get_resource() {
            let (_dir, repo) = fresh_repo(4).await;
            let url = repo
                .save_resource("https://example.com/a", "Example", "hash-aaa", 1234)
                .await
                .expect("save_resource");
            assert_eq!(
                url, "https://example.com/a",
                "save_resource must return the URL"
            );
            let found = repo
                .resource_exists_by_hash("hash-aaa")
                .await
                .expect("query");
            assert_eq!(found.as_deref(), Some("https://example.com/a"));
            assert_eq!(count_resources_by_hash(&repo, "hash-aaa").await, 1);
        }

        #[tokio::test]
        async fn test_dedup_skips_duplicate_hash() {
            let (_dir, repo) = fresh_repo(4).await;
            let first = repo
                .save_resource("https://example.com/first", "First", "dup-hash", 100)
                .await
                .expect("save_resource #1");
            let second = repo
                .save_resource("https://example.com/second", "Second", "dup-hash", 200)
                .await
                .expect("save_resource #2 (dedup)");
            assert_eq!(
                first, "https://example.com/first",
                "dedup returns existing URL"
            );
            assert_eq!(second, first, "dedup must return the SAME (existing) URL");
            assert_eq!(
                count_resources_by_hash(&repo, "dup-hash").await,
                1,
                "no duplicate row must be inserted on dedup hit"
            );
        }

        // --- save_chunk + get_vector ---

        #[tokio::test]
        async fn test_save_and_get_chunk_with_embedding() {
            let (_dir, repo) = fresh_repo(4).await;
            repo.save_resource("https://example.com/c", "C", "hash-c", 10)
                .await
                .expect("save_resource");
            let emb = vec![0.1_f32, 0.2, 0.3, 0.4];
            repo.save_chunk(
                "chunk-1",
                "https://example.com/c",
                0,
                "hello world",
                Some(&emb),
            )
            .await
            .expect("save_chunk");
            let got = repo.get_vector("chunk-1").await.expect("get_vector");
            assert_eq!(
                got.as_deref(),
                Some(emb.as_slice()),
                "embedding must round-trip through BLOB"
            );
        }

        #[tokio::test]
        async fn test_save_chunk_without_embedding_returns_none() {
            let (_dir, repo) = fresh_repo(4).await;
            repo.save_resource("https://example.com/n", "N", "hash-n", 10)
                .await
                .expect("save_resource");
            repo.save_chunk(
                "chunk-none",
                "https://example.com/n",
                0,
                "no vec here",
                None,
            )
            .await
            .expect("save_chunk");
            let got = repo.get_vector("chunk-none").await.expect("get_vector");
            assert!(
                got.is_none(),
                "chunk saved without embedding must yield None"
            );
        }

        #[tokio::test]
        async fn test_get_vector_nonexistent_chunk() {
            let (_dir, repo) = fresh_repo(4).await;
            let got = repo.get_vector("does-not-exist").await.expect("get_vector");
            assert!(
                got.is_none(),
                "missing chunk must yield Ok(None), not an error"
            );
        }

        #[tokio::test]
        async fn test_corrupt_blob_returns_error() {
            let (_dir, repo) = fresh_repo(4).await;
            repo.save_resource("https://example.com/bad", "Bad", "hash-bad", 1)
                .await
                .expect("save_resource");
            let url = "https://example.com/bad".to_string();
            let conn = repo.pool().get().await.expect("get");
            conn.interact(move |c| {
                c.execute(
                    "INSERT INTO chunks \
                     (id, resource_url, chunk_index, content, embedding_vector, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                    rusqlite::params!["bad-chunk", url, 0_i64, "", vec![1_u8, 2, 3, 4, 5]],
                )
            })
            .await
            .expect("interact")
            .expect("raw insert");
            let err = repo
                .get_vector("bad-chunk")
                .await
                .expect_err("corrupt BLOB must surface as an error");
            let msg = err.to_string();
            assert!(msg.contains("corrupto"), "missing Spanish marker: {msg}");
            assert!(msg.contains('5'), "missing length in message: {msg}");
        }

        #[tokio::test]
        async fn test_concurrent_writes_via_pool() {
            let (_dir, repo) = fresh_repo(4).await;
            let futures = (0..4_u8).map(|i| {
                let repo = repo.clone();
                async move {
                    repo.save_resource(
                        &format!("https://example.com/c{i}"),
                        "Concurrent",
                        &format!("hash-c{i}"),
                        1,
                    )
                    .await
                }
            });
            let results: Vec<_> = futures::future::join_all(futures).await;
            assert_eq!(results.len(), 4, "all 4 concurrent saves must complete");
            for r in &results {
                assert!(r.is_ok(), "concurrent save must not error: {r:?}");
            }
            for i in 0..4_u8 {
                assert_eq!(
                    count_resources_by_hash(&repo, &format!("hash-c{i}")).await,
                    1,
                    "row for hash-c{i} must exist after concurrent writes"
                );
            }
        }
    }
}
