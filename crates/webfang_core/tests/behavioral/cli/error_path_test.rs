//! Error paths: unreachable host, 404, 500 responses.

use crate::{assert_snapshot_redacted, cmd, BehavioralTest};
use std::{path::Path, time::Duration};
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
// --output-vectors without --clean-ai → exit 65, no file written (#703)
// ---------------------------------------------------------------------------

/// `webfang --output-vectors <file>` WITHOUT `--clean-ai` must fail fast with
/// exit 65 (EX_DATA) and a Spanish explanation — never run the scrape and drop
/// a 0-byte vectors file into a RAG pipeline (issue #703, class S1).
///
/// The check runs BEFORE any network I/O or sink wiring, so a live mock server
/// is mounted to prove the gate fires even when the page is perfectly
/// scrape-able: if the guard regressed, the run would succeed and create the
/// 0-byte file this test forbids. (No `.expect()` on the mock — with the
/// fail-fast in place the request never lands, and wiremock would fail the
/// test on drop over the unmet expectation.)
///
/// No `#[ignore]`: the gate fires before any ONNX model could load.
#[cfg(feature = "ai")]
mod output_vectors_without_clean_ai {
    use crate::assert_snapshot_redacted;
    use crate::BehavioralTest;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    const PAGE_HTML: &str = r#"
<html><head><title>Vector Gate Test</title></head>
<body><main><article>
<h1>Gate Me</h1>
<p>Small valid page so a regression letting the run proceed would scrape it.</p>
</article></main></body></html>
"#;

    #[tokio::test]
    async fn output_vectors_without_clean_ai_exits_65_and_writes_no_file() {
        let t = BehavioralTest::new().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
            .mount(&t.server)
            .await;

        let vectors_path = t.out.path().join("vectors.jsonl");
        let output = t
            .scraper_cmd()
            .arg("--single-page")
            .arg("--output-vectors")
            .arg(&vectors_path)
            .arg("--quiet")
            .output()
            .expect("run webfang");

        // Semantic invariants (the contract under test): exit 65 (EX_DATA), the
        // Spanish message, and — the core invariant — NO vectors file created.
        assert_eq!(
            output.status.code(),
            Some(65),
            "--output-vectors without --clean-ai must exit 65 (EX_DATA)"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("No hay vectores para exportar"),
            "stderr must carry the Spanish error, got: {stderr}"
        );
        assert_snapshot_redacted(
            "output_vectors_without_clean_ai_stderr",
            t.out.path(),
            stderr,
        );
        assert!(
            !vectors_path.exists(),
            "the vectors file must NOT be created when the flag gate rejects the run: {vectors_path:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// JS-shell content → exit 65 (EX_DATA) — honest data-format failure (#706)
// ---------------------------------------------------------------------------

/// Deterministic JS-shell body: `<div id="app">` mount point + `__NEXT_DATA__`
/// payload, well under the 50-char extraction threshold (XC-2 fixture).
const JS_SHELL_BODY: &str = "<!DOCTYPE html><html><head><title>App</title>\
             <script id=\"__NEXT_DATA__\" type=\"application/json\">{\"page\":\"/\"}</script>\
             </head><body><div id=\"app\"></div></body></html>";

/// A JS-shell page (CE-1): fetch succeeds, extraction returns <50 chars with
/// SPA markers → the run must exit 65 (DataFormatError) with the Spanish
/// per-URL failure plus the Spanish summary, never exit 69 or a fake success.
/// Drives the real binary through `BehavioralTest` (wiremock + TempDir) so it
/// exercises the full CLI funnel: extract_content guard → report_phase 65
/// routing.
///
/// #706 contract: all-fail extraction runs shift from the misleading network
/// error (69) to an honest data-format error (65).
#[tokio::test]
async fn js_shell_single_page_exits_65_with_spanish_error() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(JS_SHELL_BODY))
        .mount(&t.server)
        .await;

    let output = t
        .scraper_cmd()
        .arg("--single-page")
        .arg("--max-retries")
        .arg("0")
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert_eq!(
        output.status.code(),
        Some(65),
        "a JS-only all-fail run must exit 65 (EX_DATA)"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("extracción falló"),
        "stderr must carry the Spanish per-URL failure, got: {stderr}"
    );
    assert_snapshot_redacted("js_shell_exit_65_stderr", t.out.path(), stderr);
}

/// A mixed batch (CE-2): one legit page (≥50 chars) + one JS-shell → exit 69
/// (PartialSuccess) regardless of the extraction failure. Some content was
/// scraped, which is the dominant signal.
#[tokio::test]
async fn mixed_batch_with_js_shell_stays_69() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><head><title>Good Page</title></head><body><article>\
             <h1>Good Page</h1>\
             <p>This is a substantially long paragraph of server-rendered \
             content, comfortably over the fifty character threshold.</p>\
             </article></body></html>",
        ))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/shell"))
        .respond_with(ResponseTemplate::new(200).set_body_string(JS_SHELL_BODY))
        .mount(&t.server)
        .await;

    let output = t
        .scraper_cmd()
        .arg("--batch")
        .write_stdin(format!("{}/ok\n{}/shell\n", t.server.uri(), t.server.uri()))
        .output()
        .expect("run webfang");

    assert_eq!(
        output.status.code(),
        Some(69),
        "a mixed batch must keep PartialSuccess exit 69"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // "Batch complete" counts CRAWL successes (both pages fetched fine); the
    // JS-shell page fails later, at extraction time, via report_phase.
    assert!(
        stdout.contains("Batch complete: 2/2 succeeded"),
        "both pages were crawled, got: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("extracción falló"),
        "the extraction failure must be reported on stderr, got: {stderr}"
    );
    assert_snapshot_redacted("mixed_batch_stays_69_stderr", t.out.path(), stderr);
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
