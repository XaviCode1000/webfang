//! Regression tests for `--dry-run --batch-file` (#784).
//!
//! When `--batch-file` is provided, `--dry-run` must list the batch URLs
//! instead of performing URL discovery. Previously it reported "0 URL(s)
//! would be scraped" because `opts.url` was empty in batch mode.

use crate::cmd;
use tempfile::TempDir;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// `--dry-run --batch-file` lists batch URLs without making any HTTP requests.
#[tokio::test]
async fn dry_run_batch_file_lists_urls_without_requests() {
    let server = MockServer::start().await;

    // Reject ALL requests — dry-run must not touch the network.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("should not be fetched"))
        .expect(0)
        .named("no-requests-allowed")
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(
        &batch_file,
        format!("{}/page-a\n{}/page-b\n", server.uri(), server.uri()),
    )
    .unwrap();

    let output = cmd()
        .arg("--dry-run")
        .arg("--batch-file")
        .arg(&batch_file)
        .output()
        .expect("command must execute");

    assert!(
        output.status.success(),
        "dry-run --batch-file must succeed, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    // stdout should mention the URL count
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("2") && stdout.to_lowercase().contains("url"),
        "stdout should report 2 batch URLs, got: {stdout}"
    );

    // No HTTP requests should have been made
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        0,
        "dry-run --batch-file must NOT make HTTP requests, got {}",
        requests.len()
    );
}

/// `--dry-run --batch-file` with a single URL works the same way.
#[tokio::test]
async fn dry_run_batch_file_single_url() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("nope"))
        .expect(0)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", server.uri())).unwrap();

    let output = cmd()
        .arg("--dry-run")
        .arg("--batch-file")
        .arg(&batch_file)
        .output()
        .expect("command must execute");

    assert!(
        output.status.success(),
        "dry-run --batch-file must succeed, got {:?}",
        output.status.code()
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("1") && stdout.to_lowercase().contains("url"),
        "stdout should report 1 batch URL, got: {stdout}"
    );

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 0, "must not make HTTP requests");
}

/// `--dry-run --batch-file` produces no output files.
#[tokio::test]
async fn dry_run_batch_file_produces_zero_files() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("nope"))
        .expect(0)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", server.uri())).unwrap();

    let out = TempDir::new().unwrap();
    cmd()
        .arg("--dry-run")
        .arg("--batch-file")
        .arg(&batch_file)
        .arg("--output")
        .arg(out.path())
        .assert()
        .success();

    let entries: Vec<_> = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        entries.is_empty(),
        "dry-run --batch-file must not create output files, found {}",
        entries.len()
    );
}
