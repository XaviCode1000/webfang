//! Integration tests for SqliteVectorRepository — real I/O with temp dirs.
//!
//! Exercises CRUD operations, deduplication, vector storage, and schema
//! initialization per R-INT-01 and R-INT-03.

#![cfg(feature = "persistence")]

use webfang::domain::VectorRepository;
use webfang::infrastructure::persistence::sqlite::{
    create_memory_pool, create_pool, setup_schema, SqliteVectorRepository,
};
use tempfile::TempDir;

// ===== SCHEMA INITIALIZATION =====

/// Schema creation is idempotent — calling setup_schema twice doesn't fail.
#[tokio::test]
async fn test_setup_schema_is_idempotent() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    setup_schema(&pool).await.unwrap();
}

/// Schema creation on a temp file DB creates the file.
#[tokio::test]
async fn test_setup_schema_creates_db_file() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");
    let pool = create_pool(&db_path, 1).unwrap();
    setup_schema(&pool).await.unwrap();
    assert!(
        db_path.exists(),
        "database file should exist after schema setup"
    );
}

// ===== RESOURCE CRUD =====

/// Save a resource and retrieve its URL by content hash.
#[tokio::test]
async fn test_save_and_get_resource() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let url = repo
        .save_resource("https://example.com/page1", "Page 1", "hash_abc", 1024)
        .await
        .unwrap();

    assert_eq!(url, "https://example.com/page1");

    let existing = repo.resource_exists_by_hash("hash_abc").await.unwrap();
    assert_eq!(existing, Some("https://example.com/page1".to_string()));
}

/// resource_exists_by_hash returns None for unknown hash.
#[tokio::test]
async fn test_resource_not_exists() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let result = repo
        .resource_exists_by_hash("nonexistent_hash")
        .await
        .unwrap();
    assert_eq!(result, None);
}

// ===== DEDUPLICATION =====

/// Duplicate content hash returns existing URL without inserting a new row.
#[tokio::test]
async fn test_dedup_skips_duplicate_hash() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let url1 = repo
        .save_resource("https://a.com/page", "Page", "dedup_hash", 100)
        .await
        .unwrap();

    // Second save with same hash — should return existing URL, not error
    let url2 = repo
        .save_resource("https://b.com/page", "Page v2", "dedup_hash", 200)
        .await
        .unwrap();

    assert_eq!(url1, url2, "dedup should return the original URL");
}

// ===== CHUNK CRUD =====

/// Save a chunk with embedding and retrieve the vector.
#[tokio::test]
async fn test_save_and_get_chunk_with_embedding() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    // Save a resource first (FK constraint)
    repo.save_resource("https://example.com/doc", "Doc", "h1", 100)
        .await
        .unwrap();

    let embedding = vec![0.1, 0.2, 0.3, -0.5];
    repo.save_chunk(
        "chunk-1",
        "https://example.com/doc",
        0,
        "Hello world",
        Some(&embedding),
    )
    .await
    .unwrap();

    let retrieved = repo.get_vector("chunk-1").await.unwrap();
    assert!(retrieved.is_some(), "should retrieve embedding");
    let vec = retrieved.unwrap();
    assert_eq!(vec.len(), 4);
    assert!((vec[0] - 0.1).abs() < 1e-6);
    assert!((vec[3] - (-0.5)).abs() < 1e-6);
}

/// Save a chunk without embedding — get_vector returns None.
#[tokio::test]
async fn test_save_chunk_without_embedding() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    repo.save_resource("https://example.com/doc2", "Doc2", "h2", 100)
        .await
        .unwrap();

    repo.save_chunk(
        "chunk-2",
        "https://example.com/doc2",
        0,
        "No embedding here",
        None,
    )
    .await
    .unwrap();

    let result = repo.get_vector("chunk-2").await.unwrap();
    assert_eq!(result, None, "no embedding should return None");
}

/// get_vector for nonexistent chunk returns None.
#[tokio::test]
async fn test_get_vector_nonexistent() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let result = repo.get_vector("nonexistent").await.unwrap();
    assert_eq!(result, None);
}

// ===== IN-MEMORY REPOSITORY =====

/// from_memory() creates a working in-memory repository.
#[tokio::test]
async fn test_from_memory_repository() {
    let repo = SqliteVectorRepository::from_memory().unwrap();
    setup_schema(repo.pool()).await.unwrap();

    repo.save_resource("https://test.com/page", "Test", "mem_hash", 50)
        .await
        .unwrap();

    let exists = repo.resource_exists_by_hash("mem_hash").await.unwrap();
    assert!(exists.is_some());
}

// ===== CONCURRENT WRITES =====

/// Multiple concurrent writes via the pool don't deadlock or corrupt data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_writes_via_pool() {
    let pool = create_memory_pool().unwrap();
    setup_schema(&pool).await.unwrap();
    let repo = SqliteVectorRepository::new(pool);

    let mut handles = Vec::new();
    for i in 0..10 {
        let repo = repo.clone(); // SqliteVectorRepository is Clone (wraps Arc Pool)
        handles.push(tokio::spawn(async move {
            let url = format!("https://example.com/page{i}");
            let hash = format!("hash_{i}");
            repo.save_resource(&url, &format!("Page {i}"), &hash, (i * 100) as u64)
                .await
                .unwrap();
        }));
    }

    for handle in handles {
        handle.await.expect("concurrent write should succeed");
    }

    // Verify all resources were saved
    for i in 0..10 {
        let hash = format!("hash_{i}");
        let exists = repo.resource_exists_by_hash(&hash).await.unwrap();
        assert!(exists.is_some(), "resource {hash} should exist");
    }
}
