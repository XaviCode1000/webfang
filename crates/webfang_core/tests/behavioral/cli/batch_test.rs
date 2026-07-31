//! Batch mode: stdin and file-based URL processing.

use crate::cmd;
use crate::BehavioralTest;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::time::timeout;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// --batch (stdin)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_stdin_processes_urls() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch Stdin Test</h1>\
                 <p>Content from batch stdin processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--batch")
        .write_stdin(format!("{}\n", server.uri()))
        .timeout(Duration::from_secs(30))
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch stdin should fetch exactly the provided URL, got {} requests",
        requests.len()
    );
}

#[test]
fn batch_empty_stdin_exits_64() {
    cmd()
        .arg("--batch")
        .write_stdin("")
        .timeout(Duration::from_secs(5))
        .assert()
        .code(64)
        .stderr(predicates::str::contains("No URLs provided"));
}

// ---------------------------------------------------------------------------
// --batch-file
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_file_processes_urls() {
    let server = MockServer::start().await;
    let temp = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Batch File Test</h1>\
                 <p>Content from batch file processing.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}\n", server.uri())).unwrap();

    cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .timeout(Duration::from_secs(30))
        .assert()
        .success();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests.len(),
        1,
        "batch-file should fetch exactly the URL from the file, got {} requests",
        requests.len()
    );
}

#[test]
fn batch_empty_file_exits_64() {
    let temp = TempDir::new().unwrap();
    let batch_file = temp.path().join("urls.txt");
    std::fs::write(&batch_file, "").unwrap();

    cmd()
        .arg("--batch-file")
        .arg(&batch_file)
        .timeout(Duration::from_secs(5))
        .assert()
        .code(64)
        .stderr(predicates::str::contains("No URLs provided"));
}

// ---------------------------------------------------------------------------
// Batch timeout tests
// ---------------------------------------------------------------------------

/// A --batch-file with a slow endpoint and --timeout-secs 1 must exit 69
/// and complete well under 25s (the per-request timeout fires, not a hang).
#[tokio::test]
async fn batch_file_timeout_does_not_hang() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&t.server)
        .await;

    let batch_file = t.out.path().join("urls.txt");
    std::fs::write(&batch_file, format!("{}/slow\n", t.server.uri())).unwrap();

    let start = Instant::now();
    let output = timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            cmd()
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--timeout-secs")
                .arg("1")
                .arg("--output")
                .arg(t.out.path())
                .output()
        }),
    )
    .await
    .expect("test must not hang — tokio::time::timeout fired")
    .expect("task must not panic")
    .expect("command must execute");

    let elapsed = start.elapsed();

    assert_eq!(
        output.status.code(),
        Some(69),
        "expected exit code 69 (partial/all failures), got {:?}",
        output.status.code()
    );
    assert!(
        elapsed < Duration::from_secs(25),
        "batch timeout test should complete in under 25s, took {elapsed:?}"
    );
}

/// A --batch-file with a slow endpoint and --timeout-secs 1 must exit 69
/// and stderr must contain the failed URL and a timeout keyword.
#[tokio::test]
async fn batch_file_timeout_reports_failures() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><article><h1>Slow</h1></article></body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .mount(&t.server)
        .await;

    let batch_file = t.out.path().join("urls.txt");
    let slow_url = format!("{}/slow", t.server.uri());
    std::fs::write(&batch_file, format!("{slow_url}\n")).unwrap();

    let output = timeout(
        Duration::from_secs(30),
        tokio::task::spawn_blocking(move || {
            cmd()
                .arg("--batch-file")
                .arg(&batch_file)
                .arg("--timeout-secs")
                .arg("1")
                .arg("--output")
                .arg(t.out.path())
                .output()
        }),
    )
    .await
    .expect("test must not hang")
    .expect("task must not panic")
    .expect("command must execute");

    assert_eq!(
        output.status.code(),
        Some(69),
        "expected exit code 69, got {:?}",
        output.status.code()
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&slow_url),
        "stderr should contain the failed URL, got: {stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("timeout") || stderr.to_lowercase().contains("timed out"),
        "stderr should mention timeout, got: {stderr}"
    );
}
