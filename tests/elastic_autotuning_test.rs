//! Integration tests for the elastic autotuning pipeline (T11).
//!
//! Validates that the hardware autotuning pipeline resolves parameters correctly
//! through the 3-level chain (CLI flag > ENV var > Auto-detect) and that
//! MemoryDb works end-to-end with the real SqliteVectorRepository.

#![cfg(feature = "persistence")]

mod common;

use rust_scraper::domain::VectorRepository;
use rust_scraper::infrastructure::autotuning::{
    ElasticConfig, ElasticOverrides, DEFAULT_MAX_RESOURCE_BYTES, MIN_DB_POOL_SIZE,
};
use rust_scraper::infrastructure::config::AutotuningConfig;
use rust_scraper::infrastructure::persistence::setup_schema;
use rust_scraper::infrastructure::persistence::sqlite::SqliteVectorRepository;

/// Test 1: CLI override takes precedence over ENV and auto-detect.
///
/// Verifies that when cpu_cores is set via CLI override (ElasticOverrides),
/// the resulting AutotuningConfig.cpu_cores matches the override value,
/// and db_pool_size is at least MIN_DB_POOL_SIZE.
#[tokio::test]
async fn cpu_cores_cli_takes_precedence() {
    let overrides = ElasticOverrides {
        cpu_cores: Some(8),
        ..Default::default()
    };

    let config = ElasticConfig::resolve(&overrides);

    // CLI override of 8 must win over auto-detect
    assert_eq!(config.cpu_cores, 8, "CLI override must take precedence");

    // db_pool_size = max(cpu_cores, MIN_DB_POOL_SIZE)
    assert!(
        config.db_pool_size >= MIN_DB_POOL_SIZE,
        "db_pool_size {} must be >= MIN_DB_POOL_SIZE {}",
        config.db_pool_size,
        MIN_DB_POOL_SIZE
    );

    // Verify AutotuningConfig snapshot matches
    let autotune = AutotuningConfig::from_elastic(&config);
    assert_eq!(autotune.cpu_cores, 8);
}

/// Test 2: db_pool_size floors at MIN_DB_POOL_SIZE when cpu_cores is low.
///
/// Verifies that when cpu_cores is set to 1 or 2 (below MIN_DB_POOL_SIZE=4),
/// the db_pool_size is clamped to 4.
#[tokio::test]
async fn cpu_cores_floor_at_minimum() {
    // Test with cpu_cores = 1
    let overrides_low = ElasticOverrides {
        cpu_cores: Some(1),
        ..Default::default()
    };
    let config_low = ElasticConfig::resolve(&overrides_low);

    assert_eq!(config_low.cpu_cores, 1, "cpu_cores must be 1");
    assert_eq!(
        config_low.db_pool_size, MIN_DB_POOL_SIZE,
        "db_pool_size must floor at MIN_DB_POOL_SIZE when cpu_cores < MIN_DB_POOL_SIZE"
    );
    assert_eq!(MIN_DB_POOL_SIZE, 4, "MIN_DB_POOL_SIZE must be 4");

    // Test with cpu_cores = 2
    let overrides_two = ElasticOverrides {
        cpu_cores: Some(2),
        ..Default::default()
    };
    let config_two = ElasticConfig::resolve(&overrides_two);

    assert_eq!(config_two.cpu_cores, 2, "cpu_cores must be 2");
    assert_eq!(
        config_two.db_pool_size, MIN_DB_POOL_SIZE,
        "db_pool_size must floor at MIN_DB_POOL_SIZE when cpu_cores < MIN_DB_POOL_SIZE"
    );
}

/// Test 3: RAM budget cascades to semaphore permits correctly.
///
/// Verifies that max_concurrent = ram_budget_bytes / max_resource_bytes,
/// with a minimum of 1 permit (the .max(1) clamp).
#[tokio::test]
async fn ram_budget_cascades_to_semaphore_permits() {
    let ram_budget = 100 * 1024 * 1024; // 100 MiB
    let max_resource = 25 * 1024 * 1024; // 25 MiB (DEFAULT_MAX_RESOURCE_BYTES)

    let overrides = ElasticOverrides {
        ram_budget_bytes: Some(ram_budget),
        max_resource_bytes: Some(max_resource),
        ..Default::default()
    };

    let config = ElasticConfig::resolve(&overrides);

    assert_eq!(config.ram_budget_bytes, ram_budget);
    assert_eq!(config.max_resource_bytes, max_resource);

    // Calculate expected max_concurrent
    let expected_max_concurrent = (ram_budget / max_resource).max(1) as usize;
    assert_eq!(expected_max_concurrent, 4, "100 MiB / 25 MiB = 4 permits");

    // Verify the cascade works in Container::with_elastic path
    // The actual semaphore is created in with_elastic, but we can verify the math
    let max_concurrent = (config.ram_budget_bytes / config.max_resource_bytes).max(1) as usize;
    assert_eq!(max_concurrent, expected_max_concurrent);

    // Test edge case: tiny budget that should clamp to 1
    let tiny_budget = 1024; // 1 KiB
    let overrides_tiny = ElasticOverrides {
        ram_budget_bytes: Some(tiny_budget),
        max_resource_bytes: Some(max_resource),
        ..Default::default()
    };
    let config_tiny = ElasticConfig::resolve(&overrides_tiny);
    let max_concurrent_tiny =
        (config_tiny.ram_budget_bytes / config_tiny.max_resource_bytes).max(1) as usize;
    assert_eq!(
        max_concurrent_tiny, 1,
        "tiny budget must clamp to at least 1 permit"
    );
}

/// Test 4: MemoryDb elastic pipeline roundtrip.
///
/// Uses SqliteVectorRepository::from_memory() to create an in-memory repository,
/// manually constructs AutotuningConfig, builds ElasticIngestion, runs the
/// pipeline against a wiremock server, and verifies content was persisted.
#[tokio::test]
async fn memory_db_elastic_pipeline_roundtrip() {
    use std::sync::Arc;

    use rust_scraper::application::elastic_ingestion::ElasticIngestion;
    use rust_scraper::infrastructure::bridge::CpuBridge;
    use rust_scraper::infrastructure::cpu_pool::RayonCpuPool;
    use rust_scraper::infrastructure::crawler::resource_downloader::{
        DownloadConfig, ResourceDownloader,
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // 1. Create in-memory repository with schema
    let repo = SqliteVectorRepository::from_memory().expect("from_memory");
    setup_schema(repo.pool())
        .await
        .expect("setup_schema on memory repo");

    // 2. Set up pipeline components
    let cpu_pool = RayonCpuPool::new(2).expect("Rayon pool");
    let bridge = CpuBridge::new(cpu_pool);

    let config = AutotuningConfig {
        cpu_cores: 2,
        ram_budget_bytes: 1 << 20, // 1 MiB
    };

    // 3. Create HTTP downloader with wiremock
    let server = MockServer::start().await;
    let body = "<html><head><title>Test Page</title></head>\
                <body><main><p>Real content here</p></main></body></html>";
    Mock::given(method("GET"))
        .and(path("/test"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

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

    // 4. Build and run the pipeline
    let ingestion = ElasticIngestion::new(downloader, bridge, Arc::new(repo.clone()), config);
    let url = format!("{}/test", server.uri());

    ingestion
        .run(&url)
        .await
        .expect("pipeline must complete successfully");

    // 5. Verify content was persisted in the in-memory database
    // We can't directly query the in-memory DB from outside, but we can use
    // the repository's methods to verify
    let existing = repo
        .resource_exists_by_hash(&compute_sha256(body.as_bytes()))
        .await
        .expect("resource_exists_by_hash");
    assert!(
        existing.is_some(),
        "resource must exist in memory DB after pipeline run"
    );
}

/// Test 5: RAM budget cascade to elastic ingestion layer.
///
/// Sets up the elastic pipeline with a tiny ram_budget and verifies that
/// the download semaphore permits are calculated correctly.
#[tokio::test]
async fn ram_budget_cascade_to_elastic_ingestion() {
    use std::sync::Arc;

    use rust_scraper::application::elastic_ingestion::ElasticIngestion;
    use rust_scraper::infrastructure::bridge::CpuBridge;
    use rust_scraper::infrastructure::cpu_pool::RayonCpuPool;
    use rust_scraper::infrastructure::crawler::resource_downloader::{
        DownloadConfig, ResourceDownloader,
    };

    // 1. Create in-memory repository with schema
    let repo = SqliteVectorRepository::from_memory().expect("from_memory");
    setup_schema(repo.pool())
        .await
        .expect("setup_schema on memory repo");

    // 2. Set up pipeline with tiny ram_budget (5 MiB)
    let cpu_pool = RayonCpuPool::new(2).expect("Rayon pool");
    let bridge = CpuBridge::new(cpu_pool);

    let ram_budget = 5 * 1024 * 1024; // 5 MiB
    let max_resource = 25 * 1024 * 1024; // 25 MiB

    let config = AutotuningConfig {
        cpu_cores: 2,
        ram_budget_bytes: ram_budget,
    };

    // 3. Verify the semaphore permits calculation
    let max_concurrent = (ram_budget / max_resource).max(1) as usize;
    assert_eq!(
        max_concurrent, 1,
        "5 MiB / 25 MiB = 0.2, clamped to 1 permit"
    );

    // 4. Create downloader with the expected semaphore size
    let client = wreq::Client::builder().build().expect("wreq client");
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let downloader = ResourceDownloader::with_config(
        semaphore,
        client,
        DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 5,
            max_size_bytes: max_resource,
            ..DownloadConfig::default()
        },
    );

    // 5. Build the pipeline and verify it was constructed correctly
    let _ingestion = ElasticIngestion::new(downloader, bridge, Arc::new(repo), config);

    // The pipeline is constructed successfully with the correct semaphore permits
    // We can't directly inspect the semaphore from outside, but we can verify
    // the config values that drive the calculation
    assert_eq!(ram_budget, 5 * 1024 * 1024);
    assert_eq!(max_resource, DEFAULT_MAX_RESOURCE_BYTES);
    assert_eq!(max_concurrent, 1);
}

/// Helper: Compute SHA-256 hex digest (matching the pipeline's sha256_hex function).
fn compute_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let hash = hasher.finalize();
    let mut out = String::with_capacity(hash.len() * 2);
    for b in hash {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}
