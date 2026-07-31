//! Integration test for `create_memory_pool` and `MemoryDb` test helper.
//!
//! Validates that an in-memory SQLite database can be created, schema
//! initialised, and data round-tripped without touching disk.

#![cfg(feature = "persistence")]

mod common;

use webfang::domain::VectorRepository;
use webfang::infrastructure::persistence::{
    create_memory_pool, setup_schema, SqliteVectorRepository,
};

/// Smoke-test: `create_memory_pool` returns a working pool and the schema can
/// be initialised on it.
#[tokio::test]
async fn test_memory_pool_schema_init() {
    let pool = create_memory_pool().expect("create_memory_pool");
    setup_schema(&pool).await.expect("setup_schema on :memory:");
}

/// Round-trip: create a table, insert a row, query it back.
#[tokio::test]
async fn test_memory_db_roundtrip() {
    let mem = common::MemoryDb::new();
    let conn = mem.pool().get().await.expect("get connection");

    // 1. Create a test table
    conn.interact(|c| {
        c.execute_batch("CREATE TABLE IF NOT EXISTS kv (key TEXT PRIMARY KEY, value TEXT);")
    })
    .await
    .expect("interact")
    .expect("create table");

    // 2. Insert a row
    conn.interact(|c| {
        c.execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)",
            rusqlite::params!["hello", "world"],
        )
    })
    .await
    .expect("interact")
    .expect("insert");

    // 3. Query it back
    let value: String = conn
        .interact(|c| {
            c.query_row(
                "SELECT value FROM kv WHERE key = ?1",
                rusqlite::params!["hello"],
                |row| row.get(0),
            )
        })
        .await
        .expect("interact")
        .expect("query");

    assert_eq!(value, "world", "in-memory roundtrip must match");
}

/// Verify `SqliteVectorRepository::from_memory()` produces a usable repo.
#[tokio::test]
async fn test_repository_from_memory() {
    let repo = SqliteVectorRepository::from_memory().expect("from_memory");
    setup_schema(repo.pool())
        .await
        .expect("setup_schema on memory repo");

    // Insert + dedup roundtrip via the trait methods.
    let url = repo
        .save_resource("https://example.com/mem", "Mem", "hash-mem", 42)
        .await
        .expect("save_resource");
    assert_eq!(url, "https://example.com/mem");

    // Dedup: same hash, different URL → must return the original.
    let dup = repo
        .save_resource("https://example.com/dup", "Dup", "hash-mem", 99)
        .await
        .expect("dedup");
    assert_eq!(dup, "https://example.com/mem");
}

/// `MemoryDb::into_pool` transfers ownership correctly.
#[tokio::test]
async fn test_memory_db_into_pool() {
    let mem = common::MemoryDb::new();
    let pool = mem.into_pool();
    // Pool is still usable after ownership transfer.
    let conn = pool.get().await.expect("get after into_pool");
    conn.interact(|c| c.execute_batch("SELECT 1"))
        .await
        .expect("interact")
        .expect("simple query");
}
