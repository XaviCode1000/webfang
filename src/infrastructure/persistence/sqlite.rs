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

use std::path::Path;

use deadpool_sqlite::{Config, Hook, HookError, Manager, Pool, Runtime};

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
}
