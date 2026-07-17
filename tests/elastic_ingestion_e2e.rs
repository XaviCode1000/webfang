#![cfg(feature = "persistence")]

//! End-to-end integration test for the elastic ingestion pipeline (PR5, Issue #51).
//!
//! Spins up a `wiremock` HTTP server, runs the full 7-layer pipeline
//! (`ElasticIngestion::run`) against a **real** SQLite database on a `tempfile`,
//! and verifies the cleaned content is persisted as a resource + chunk with a
//! `NULL` embedding.
//!
//! # Feature coverage
//!
//! This test is feature-agnostic: it runs under the default build AND under
//! `--features ai`. Under `ai`, the ONNX `with_cleaner` / `cleaner_chunks` code
//! path COMPILES (structurally verifying Decision 5's wiring) but is NOT
//! exercised at runtime — the `SemanticCleaner` trait is sealed (no test impl)
//! and `SemanticCleanerImpl::new` eagerly loads a ~90 MB model, so the no-cleaner
//! `lol_html` text path is used. The embedding column is therefore `NULL`.
//!
//! See PR5 apply-progress for the full rationale.

use std::sync::Arc;

use webfang::application::elastic_ingestion::ElasticIngestion;
use webfang::infrastructure::bridge::CpuBridge;
use webfang::infrastructure::config::AutotuningConfig;
use webfang::infrastructure::cpu_pool::RayonCpuPool;
use webfang::infrastructure::crawler::resource_downloader::{
    DownloadConfig, ResourceDownloader,
};
use webfang::infrastructure::persistence::sqlite::{
    create_pool, setup_schema, SqliteVectorRepository,
};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Count rows in a table matching `WHERE <column> = url`.
///
/// The `resources` table keys on `url`; the `chunks` table keys on
/// `resource_url` (FK to `resources.url`) — hence the column parameter.
fn count_rows(conn: &rusqlite::Connection, table: &str, column: &str, url: &str) -> i64 {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1");
    conn.query_row(&sql, rusqlite::params![url], |r| r.get::<_, i64>(0))
        .unwrap_or_else(|e| panic!("contar filas en {table}.{column}: {e}"))
}

#[tokio::test]
async fn elastic_ingestion_persists_cleaned_content_to_sqlite() {
    // ---- Arrange: real SQLite DB on a tempfile ----
    let dir = TempDir::new().expect("directorio temporal");
    let db_path = dir.path().join("elastic_e2e.db");
    let pool = create_pool(&db_path, 4).expect("pool SQLite");
    setup_schema(&pool).await.expect("esquema SQLite");
    let repo = SqliteVectorRepository::new(pool);

    // ---- Arrange: pipeline components (wreq downloader + Rayon bridge) ----
    let client = wreq::Client::builder()
        .build()
        .expect("fallo construyendo cliente wreq");
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1 << 20));
    let downloader = ResourceDownloader::with_config(
        semaphore,
        client,
        DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024 * 1024,
            ..DownloadConfig::default()
        },
    );
    let bridge = CpuBridge::new(RayonCpuPool::new(2).expect("pool Rayon de 2 hilos"));
    let config = AutotuningConfig {
        cpu_cores: 2,
        ram_budget_bytes: 1 << 20,
    };
    let orc = ElasticIngestion::new(downloader, bridge, repo, config);

    // ---- Arrange: wiremock serving boilerplate-laden HTML ----
    let body = "<nav>site navigation menu</nav>\
                <main><article>\
                    <h1>E2E Title</h1>\
                    <p>real persisted content here</p>\
                </article></main>\
                <footer>copyright notice 2026</footer>";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.as_bytes().to_vec()))
        .mount(&server)
        .await;
    let url = format!("{}/", server.uri());

    // ---- Act: run the full pipeline ----
    orc.run(&url)
        .await
        .expect("el pipeline debe completarse sin error");

    // ---- Assert: resource + chunk persisted in real SQLite ----
    let conn = rusqlite::Connection::open(&db_path).expect("abrir SQLite para verificación");
    assert_eq!(
        count_rows(&conn, "resources", "url", &url),
        1,
        "exactamente un recurso persistido"
    );
    let chunk_count = count_rows(&conn, "chunks", "resource_url", &url);
    assert!(
        chunk_count >= 1,
        "al menos un chunk persistido, obtuve {chunk_count}"
    );

    let content: String = conn
        .query_row(
            "SELECT content FROM chunks WHERE resource_url = ?1 ORDER BY chunk_index LIMIT 1",
            rusqlite::params![&url],
            |r| r.get::<_, String>(0),
        )
        .expect("leer contenido del chunk");
    assert!(
        content.contains("real persisted content here"),
        "contenido limpio persistido: {content}"
    );
    assert!(
        !content.contains("site navigation menu"),
        "el boilerplate <nav> no debe persistir: {content}"
    );
    assert!(
        !content.contains("copyright notice"),
        "el boilerplate <footer> no debe persistir: {content}"
    );

    // Embedding is NULL: no ONNX cleaner was wired (Decision 5's wiring compiles
    // under `--features ai` but isn't runtime-exercised here).
    let embedding_blob: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding_vector FROM chunks WHERE resource_url = ?1 LIMIT 1",
            rusqlite::params![&url],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .expect("leer embedding del chunk");
    assert!(
        embedding_blob.is_none(),
        "embedding debe ser NULL sin limpiador ONNX"
    );
}

/// Dedup short-circuit at the persistence layer: re-ingesting the same content
/// must NOT create duplicate resource/chunk rows (frozen Decision 3).
#[tokio::test]
async fn elastic_ingestion_dedup_prevents_duplicate_rows() {
    let dir = TempDir::new().expect("directorio temporal");
    let db_path = dir.path().join("elastic_dedup.db");
    let pool = create_pool(&db_path, 4).expect("pool SQLite");
    setup_schema(&pool).await.expect("esquema SQLite");
    let repo = SqliteVectorRepository::new(pool);

    let client = wreq::Client::builder().build().expect("wreq client");
    let semaphore = Arc::new(tokio::sync::Semaphore::new(1 << 20));
    let downloader = ResourceDownloader::with_config(
        semaphore,
        client,
        DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: 1024 * 1024,
            ..DownloadConfig::default()
        },
    );
    let bridge = CpuBridge::new(RayonCpuPool::new(2).expect("pool"));
    let orc = ElasticIngestion::new(
        downloader,
        bridge,
        repo,
        AutotuningConfig {
            cpu_cores: 2,
            ram_budget_bytes: 1 << 20,
        },
    );

    let body = "<main><p>duplicate me</p></main>";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.as_bytes().to_vec()))
        .mount(&server)
        .await;
    let url = format!("{}/", server.uri());

    orc.run(&url).await.expect("primera ingestión");
    orc.run(&url).await.expect("segunda ingestión (dedup)");

    let conn = rusqlite::Connection::open(&db_path).expect("abrir SQLite");
    assert_eq!(
        count_rows(&conn, "resources", "url", &url),
        1,
        "dedup: exactamente un recurso tras dos ingestiones idénticas"
    );
    assert_eq!(
        count_rows(&conn, "chunks", "resource_url", &url),
        1,
        "dedup: exactamente un chunk tras dos ingestiones idénticas"
    );
}
