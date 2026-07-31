//! Integration tests for `ResourceDownloader` (elastic-ingestion-51, PR2).
//!
//! Exercises the public download surface end-to-end against a wiremock
//! `MockServer`, including a shared-semaphore concurrency scenario.

use webfang::infrastructure::crawler::{DownloadConfig, ResourceDownloader};
use std::sync::Arc;
use tokio::sync::Semaphore;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A realistic HTML page used as the download payload.
const HTML_PAGE: &[u8] = b"<html><head><title>WebFang</title></head>\
<body><h1>Foo</h1><p>bar baz qux</p></body></html>";

/// End-to-end download against a wiremock server returns the exact bytes.
#[tokio::test]
async fn test_end_to_end_download() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(HTML_PAGE.to_vec()))
        .mount(&mock_server)
        .await;

    let config = DownloadConfig {
        global_timeout_seconds: 5,
        chunk_timeout_seconds: 5,
        max_size_bytes: 1024 * 1024,
        ..DownloadConfig::default()
    };
    let semaphore = Arc::new(Semaphore::new(1 << 20));
    let client = wreq::Client::builder()
        .build()
        .expect("fallo construyendo cliente wreq de prueba");
    let downloader = ResourceDownloader::with_config(semaphore, client, config);

    let url = format!("{}/", mock_server.uri());
    let bytes = downloader
        .download(&url)
        .await
        .expect("la descarga end-to-end debió exitosa");

    assert_eq!(
        bytes, HTML_PAGE,
        "los bytes descargados no coinciden con el cuerpo"
    );
}

/// Several concurrent downloads sharing a semaphore all complete with correct
/// bytes, and permits are conserved exactly (`available == budget` afterwards).
///
/// With per-chunk byte-weighted acquire, each 64 KiB download holds up to 64 KiB
/// of permits while in flight. The budget is sized for two concurrent downloads
/// (2 × 64 KiB = 128 KiB), so the remaining three serialize behind backpressure
/// until in-flight ones release. Because each download acquires at most its body
/// size and the budget is `2 × body`, the semaphore is deadlock-free (the budget
/// can only be fully held once two downloads are *done*, about to release).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_downloads_under_semaphore() {
    let mock_server = MockServer::start().await;
    // 64 KiB body per download.
    let body = vec![0xA5u8; 64 * 1024];
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&mock_server)
        .await;

    let config = DownloadConfig::default();
    // Budget for 2 concurrent 64 KiB downloads (2 × body size). This is the
    // tightest deadlock-free budget: each download acquires ≤ body size, so the
    // budget can only be exhausted by two *completed* downloads (about to drop).
    let budget = 2 * body.len();
    let semaphore = Arc::new(Semaphore::new(budget));
    let client = wreq::Client::builder()
        .build()
        .expect("fallo construyendo cliente wreq de prueba");
    let downloader = Arc::new(ResourceDownloader::with_config(
        Arc::clone(&semaphore),
        client,
        config,
    ));

    let url = format!("{}/", mock_server.uri());

    let mut handles = Vec::new();
    for _ in 0..5 {
        let dl = Arc::clone(&downloader);
        let url = url.clone();
        handles.push(tokio::spawn(async move { dl.download(&url).await }));
    }

    let mut successes = 0usize;
    for handle in handles {
        let bytes = handle
            .await
            .expect("join tarea de descarga")
            .expect("descarga concurrente debió exitosa");
        assert_eq!(bytes, body, "los bytes descargados no coinciden");
        successes += 1;
    }

    assert_eq!(
        successes, 5,
        "las 5 descargas concurrentes debieron completarse"
    );

    // Exact conservation: no permit leak (available < budget) and no inflation
    // (available > budget). This is the integration guard against the PR2
    // double-release / missing per-chunk acquire bug.
    let available = semaphore.available_permits();
    assert_eq!(
        available, budget,
        "permisos no conservados: available {available} != budget {budget}"
    );
}
