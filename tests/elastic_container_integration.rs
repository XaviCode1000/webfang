//! Integration tests for the elastic ingestion Container wiring.
//!
//! Verifies that the Container correctly activates the elastic pipeline
//! when `--elastic` is set, and gracefully skips it otherwise.

use rust_scraper::application::container::Container;
use rust_scraper::domain::CrawlerConfig;
use rust_scraper::infrastructure::autotuning::ElasticOverrides;
use rust_scraper::infrastructure::config::ScraperConfig;
use tempfile::TempDir;

#[tokio::test]
async fn container_with_elastic_builds_pipeline() {
    let dir = TempDir::new().expect("directorio temporal");
    let seed = url::Url::parse("https://example.com").expect("url valida");
    let crawler_config = CrawlerConfig::new(seed);
    let scraper_config = ScraperConfig {
        output_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let overrides = ElasticOverrides {
        cpu_cores: Some(2),
        ram_budget_bytes: Some(512 * 1024 * 1024), // 512 MB
        db_path: Some(dir.path().join("elastic.db")),
        ..Default::default()
    };

    let container = Container::new(crawler_config, scraper_config)
        .await
        .expect("container base")
        .with_elastic(&overrides)
        .await
        .expect("container con pipeline elástico");

    let elastic = container.elastic_ingestion();
    assert!(
        elastic.is_some(),
        "elastic_ingestion() debe retornar Some después de with_elastic()"
    );
}

#[tokio::test]
async fn container_without_elastic_is_none() {
    let dir = TempDir::new().expect("directorio temporal");
    let seed = url::Url::parse("https://example.com").expect("url valida");
    let crawler_config = CrawlerConfig::new(seed);
    let scraper_config = ScraperConfig {
        output_dir: dir.path().to_path_buf(),
        ..Default::default()
    };

    let container = Container::new(crawler_config, scraper_config)
        .await
        .expect("container base");

    assert!(
        container.elastic_ingestion().is_none(),
        "elastic_ingestion() debe ser None sin with_elastic()"
    );
}

#[tokio::test]
async fn elastic_pipeline_returns_error_on_bad_url() {
    use std::sync::Arc;

    use rust_scraper::application::elastic_ingestion::ElasticIngestion;
    use rust_scraper::infrastructure::autotuning::ElasticConfig;
    use rust_scraper::infrastructure::bridge::CpuBridge;
    use rust_scraper::infrastructure::config::AutotuningConfig;
    use rust_scraper::infrastructure::cpu_pool::RayonCpuPool;
    use rust_scraper::infrastructure::crawler::resource_downloader::{
        DownloadConfig, ResourceDownloader,
    };
    use rust_scraper::infrastructure::persistence::sqlite::{self, setup_schema};

    let dir = TempDir::new().expect("directorio temporal");
    let db_path = dir.path().join("elastic_test.db");

    let config = ElasticConfig {
        cpu_cores: 2,
        ram_budget_bytes: 256 * 1024 * 1024,
        max_resource_bytes: 25 * 1024 * 1024,
        db_pool_size: 4,
        db_path: db_path.clone(),
    };

    let cpu_pool = RayonCpuPool::new(2).expect("pool rayon");
    let bridge = CpuBridge::new(cpu_pool);

    let pool = sqlite::create_pool(&db_path, 4).expect("pool sqlite");
    setup_schema(&pool).await.expect("esquema sqlite");
    let repo = sqlite::SqliteVectorRepository::new(pool);

    let client = wreq::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("cliente wreq");
    let semaphore = Arc::new(tokio::sync::Semaphore::new(10));
    let downloader = ResourceDownloader::with_config(
        semaphore,
        client,
        DownloadConfig {
            global_timeout_seconds: 5,
            chunk_timeout_seconds: 3,
            ..DownloadConfig::default()
        },
    );

    let autotune = AutotuningConfig::from_elastic(&config);
    let ingestion = ElasticIngestion::new(downloader, bridge, repo, autotune);

    // URL pointing to nowhere — should return an error, not panic
    let result = ingestion.run("http://127.0.0.1:1/nonexistent").await;
    assert!(
        result.is_err(),
        "URL inválida debe retornar error, no panic"
    );
}
