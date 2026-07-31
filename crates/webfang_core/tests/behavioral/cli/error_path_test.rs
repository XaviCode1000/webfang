//! Error paths: unreachable host, 404, 500 responses.

use crate::assert_snapshot_redacted;
use crate::cmd;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Unreachable host → exit error, timeout message
// ---------------------------------------------------------------------------

#[test]
fn unreachable_host_exits_error() {
    cmd()
        .arg("--url")
        .arg("http://127.0.0.1:1")
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("2")
        .arg("--max-retries")
        .arg("0")
        .arg("--quiet")
        .assert()
        .failure();
}

#[test]
fn unreachable_host_exit_code_69() {
    cmd()
        .arg("--url")
        .arg("http://127.0.0.1:1")
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("2")
        .arg("--max-retries")
        .arg("0")
        .arg("--quiet")
        .assert()
        .code(69);
}

#[test]
fn unreachable_host_stderr_mentions_failure() {
    let output = cmd()
        .arg("--url")
        .arg("http://127.0.0.1:1")
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("2")
        .arg("--max-retries")
        .arg("0")
        .arg("--quiet")
        .output()
        .expect("run binary");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_snapshot_redacted("unreachable_host_stderr", Path::new("__no_temp__"), stderr);
}

// ---------------------------------------------------------------------------
// Unknown TLS profile → exit 78 (EX_CONFIG), Spanish message
// ---------------------------------------------------------------------------

/// An unrecognized `--h2-profile` is a configuration error: the run must abort
/// with exit code 78 (`EX_CONFIG`) and a Spanish user-facing message, before any
/// network I/O happens.
#[test]
fn unknown_h2_profile_exits_78_with_spanish_message() {
    // No wiremock / TempDir needed: the profile is rejected inside
    // `build_http_client_config` BEFORE any fetch or output write, so the run is
    // fully hermetic. `cmd()` (not `BehavioralTest`) matches the other
    // no-network error paths in this file (e.g. `unreachable_host_*`).
    let output = cmd()
        .arg("--url")
        .arg("http://example.com")
        .arg("--single-page")
        .arg("--h2-profile")
        .arg("Firefox")
        .arg("--quiet")
        .output()
        .expect("run webfang");

    // Semantic invariants (the contract under test): exit 78 (EX_CONFIG) and the
    // Spanish message. Asserted explicitly so they hold even if the snapshot
    // below drifts.
    assert_eq!(
        output.status.code(),
        Some(78),
        "unknown TLS profile must exit 78 (EX_CONFIG)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Perfil TLS desconocido"),
        "stderr must carry the Spanish message, got: {stderr}"
    );

    // Snapshot the full stderr for regression detection, routed through the
    // crate-root helper so it lands in `tests/behavioral/snapshots/` alongside
    // the other behavioral snapshots (insta keys the on-disk location off the
    // module where `assert_snapshot!` expands). The helper redacts tracing
    // timestamps / source line numbers. The valid-profile catalog
    // (`Opciones válidas: ...`) is captured verbatim: it is pinned by
    // `Cargo.lock` (stable run-to-run), and a `wreq-util` upgrade that adds
    // profiles is exactly the kind of change worth a human snapshot review.
    // `Path::new("__no_temp__")` is a non-matching placeholder: this run passes
    // no `--output`, so there is no temp dir to redact.
    assert_snapshot_redacted(
        "unknown_h2_profile_stderr",
        Path::new("__no_temp__"),
        stderr,
    );
}

// ---------------------------------------------------------------------------
// Slow server → timeout → exit error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn slow_server_timeout_exits_error() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body>slow</body></html>")
                .set_delay(Duration::from_secs(10)),
        )
        .expect(1)
        .mount(&server)
        .await;

    let result = cmd()
        .arg("--url")
        .arg(format!("{}/slow", server.uri()))
        .arg("--single-page")
        .arg("--timeout-secs")
        .arg("1")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert_eq!(
        result.status.code(),
        Some(69),
        "slow server should time out with exit code 69"
    );
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert_snapshot_redacted("slow_server_timeout_stderr", output.path(), stderr);
}

// ---------------------------------------------------------------------------
// 404 response → exit error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn not_found_response_exits_error() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/missing", server.uri()))
        .arg("--single-page")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .failure();
}

#[tokio::test]
async fn not_found_response_exit_code_nonzero() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&server)
        .await;

    let output_result = cmd()
        .arg("--url")
        .arg(format!("{}/missing", server.uri()))
        .arg("--single-page")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert_ne!(
        output_result.status.code(),
        Some(0),
        "404 response should produce a non-zero exit code"
    );
}

// ---------------------------------------------------------------------------
// 500 response → exit error
// ---------------------------------------------------------------------------

#[tokio::test]
async fn server_error_response_exits_error() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/error"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(format!("{}/error", server.uri()))
        .arg("--single-page")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .assert()
        .failure();
}

#[tokio::test]
async fn server_error_response_exit_code_nonzero() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();

    Mock::given(method("GET"))
        .and(path("/error"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(&server)
        .await;

    let output_result = cmd()
        .arg("--url")
        .arg(format!("{}/error", server.uri()))
        .arg("--single-page")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(output.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    assert_ne!(
        output_result.status.code(),
        Some(0),
        "500 response should produce a non-zero exit code"
    );
}

// ---------------------------------------------------------------------------
// --output-dir flag
// ---------------------------------------------------------------------------

/// --output-dir flag is accepted and output is written there.
#[tokio::test]
async fn output_dir_flag_creates_directory() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let custom_dir = output.path().join("custom_out");

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article>\
                 <h1>Custom Dir Test</h1>\
                 <p>Verify output goes to the specified directory.</p>\
                 </article></body></html>",
        ))
        .expect(1)
        .mount(&server)
        .await;

    cmd()
        .arg("--url")
        .arg(server.uri())
        .arg("--single-page")
        .arg("--output")
        .arg(&custom_dir)
        .arg("--quiet")
        .assert()
        .success();

    assert!(
        custom_dir.exists(),
        "--output directory should be created by the scraper"
    );
}
