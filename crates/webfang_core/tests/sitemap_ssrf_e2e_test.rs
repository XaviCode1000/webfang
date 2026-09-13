//! E2E regression suite: sitemap-discovery SSRF entry guard (#1382, #1376).
//!
//! `--use-sitemap` against a loopback listener produced 18 real requests
//! with every guard armed (measured in #1376): `build_discovery_client`
//! installs the validating resolver and redirect policy but nothing on the
//! sitemap path called `reject_forbidden_literal_url`, so the AGENTS.md
//! guard-chain order (entry validation before any fetch) was violated. This
//! suite pins the restored order end to end through the REAL `webfang`
//! binary (debug profile, production posture):
//!
//! | Row | Target                                  | Pinned outcome |
//! |-----|-----------------------------------------|----------------|
//! | 1   | loopback seed + `--use-sitemap`         | exit 69, Spanish typed SSRF error, 0 outbound |
//! | 2   | hostname seed + explicit forbidden-lit  | exit 69, Spanish typed SSRF error, 0 outbound |
//!
//! Determinism notes (test-quality rule 6):
//! - Wiremock binds `127.0.0.1` as the zero-outbound tripwire; every rejected
//!   URL literal is an address the OS never dials, so no real network is
//!   touched regardless of host routing tables.
//! - Row 1 re-verifies the #1376 measurement posture: pre-fix, the discovery
//!   chain dials the mock (robots.txt GET, HEAD/GET probes) and the journal
//!   is non-empty — that is the RED. Post-fix the journal must be empty and
//!   the CLI must fail with the typed SSRF rejection (exit 69).

#[path = "common/cli_harness.rs"]
mod common;

use std::process::Output;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const RUN_TIMEOUT: Duration = Duration::from_secs(120);

/// Normalize captured stderr: drop ESC bytes and collapse whitespace runs.
fn normalize_captured(s: &str) -> String {
    s.replace('\x1b', "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run `webfang --use-sitemap` in production posture (entry guard enforced —
/// the harness default is disarmed, so remove it explicitly) against `seed`,
/// returning the output plus the tripwire mock that must observe zero
/// requests when the guard fires pre-socket.
async fn run_production_sitemap(seed: &str, sitemap_url: Option<&str>) -> (Output, MockServer) {
    let mock_server = MockServer::start().await;
    // Tripwire: if any request reached the network, the mock would record it.
    // Every route the discovery chain probes (robots.txt, /sitemap.xml, the
    // well-known fallbacks, sub-paths) is served the same body so a pre-fix
    // run reaches the journal instead of 404-ing silently.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\
             <url><loc>http://never-reached.example/page</loc></url></urlset>",
        ))
        .mount(&mock_server)
        .await;
    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock_server)
        .await;

    let out_dir = tempfile::TempDir::new().expect("temp output dir");
    let mut cmd = common::cmd();
    cmd.env_remove(webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV)
        .arg("--url")
        .arg(seed)
        .arg("--use-sitemap")
        .arg("--output")
        .arg(out_dir.path())
        .arg("--timeout-secs")
        .arg("5")
        .arg("--max-retries")
        .arg("0");
    if let Some(s) = sitemap_url {
        cmd.arg("--sitemap-url").arg(s);
    }
    let output = cmd.timeout(RUN_TIMEOUT).output().expect("spawn webfang");
    (output, mock_server)
}

/// Assert the pinned rejection contract for a forbidden sitemap-path target:
/// exit 69 (EX_UNAVAILABLE — the CLI classifies the typed InvalidUrl as a
/// hard scrape failure), the Spanish SSRF message naming the offending
/// address, and zero outbound requests.
async fn assert_rejected_sitemap(seed: &str, sitemap_url: Option<&str>, expected_ip: &str) {
    let (output, mock_server) = run_production_sitemap(seed, sitemap_url).await;
    let stderr = normalize_captured(&String::from_utf8_lossy(&output.stderr));

    assert!(
        !output.status.success(),
        "forbidden sitemap target must fail: seed={seed} stderr={stderr}"
    );
    let code = output.status.code().unwrap_or(0);
    assert_eq!(code, 69, "exit code contract (EX_UNAVAILABLE): {stderr}");
    assert!(
        stderr.contains("SSRF detectado") && stderr.contains(expected_ip),
        "typed Spanish rejection naming the offending IP missing. \
         stderr: {stderr}"
    );
    let requests = mock_server
        .received_requests()
        .await
        .expect("wiremock request journal");
    assert!(
        requests.is_empty(),
        "entry guard fired after a socket opened — the discovery chain dialed \
         the mock {} times; pre-socket rejection requires zero requests",
        requests.len()
    );
}

// ============================================================================
// Row 1 — loopback seed + --use-sitemap (the #1376 measured bypass)
// ============================================================================

/// The exact measured posture from #1376: `--use-sitemap` against a local
/// listener opened 18 requests with every guard armed. Post-fix the seed is
/// cut at the sitemap-discovery entry, pre-socket, so the run fails with the
/// typed SSRF rejection and the listener observes zero requests.
#[tokio::test]
async fn row1_sitemap_discovery_loopback_seed_is_rejected_pre_socket() {
    let (output, mock_server) = run_production_sitemap("http://127.0.0.1:59998/", None).await;
    let stderr = normalize_captured(&String::from_utf8_lossy(&output.stderr));

    assert!(
        !output.status.success(),
        "loopback seed with --use-sitemap must fail: {stderr}"
    );
    assert_eq!(
        output.status.code(),
        Some(69),
        "exit code contract (EX_UNAVAILABLE): {stderr}"
    );
    assert!(
        stderr.contains("SSRF detectado") && stderr.contains("127.0.0.1"),
        "typed Spanish rejection naming the seed IP missing. stderr: {stderr}"
    );
    let requests = mock_server
        .received_requests()
        .await
        .expect("wiremock request journal");
    assert!(
        requests.is_empty(),
        "the #1376 bypass re-manifested: the discovery chain dialed the \
         listener {} times; the entry guard must cut the seed pre-socket",
        requests.len()
    );
}

// ============================================================================
// Row 2 — hostname seed + explicit forbidden-literal sitemap URL
// ============================================================================

/// The explicit `--sitemap-url` target is validated at entry even when the
/// seed is a hostname: the sitemap fetch path must pay the same entry guard
/// as every other fetch surface. The literal sits on port 9 (discard —
/// nothing listens), so a followed fetch surfaces a connect error, never a
/// 200, making stopped-vs-followed distinguishable.
#[tokio::test]
async fn row2_explicit_sitemap_url_literal_is_rejected_pre_socket() {
    assert_rejected_sitemap(
        "https://example.com",
        Some("http://192.168.1.5:9/sitemap.xml"),
        "192.168.1.5",
    )
    .await;
}

// ============================================================================
// Positive control — the harness network genuinely reaches (row 9 parity)
// ============================================================================

/// Proves row 1/2 rejections are policy, not a dead test network: with ONLY
/// the documented entry-layer disarmer set (the posture every other CLI
/// test in this repo runs), a loopback `--use-sitemap` run reaches the
/// listener and completes normally.
#[tokio::test]
async fn row3_positive_control_sitemap_reachable_when_entry_guard_disarmed() {
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("User-agent: *\nSitemap: /sitemap.xml\n"),
        )
        .mount(&mock_server)
        .await;
    let xml = format!(
        r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{}/page</loc></url>
</urlset>"#,
        mock_server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(xml)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(&mock_server)
        .await;
    // Serve the listed page too: the positive control must reach the full
    // chain (robots → sitemap → scrape), not stop at a 404 that masquerades
    // as a failure. Content clears the 50-char minimum-content guard.
    Mock::given(method("GET"))
        .and(path("/page"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html><body><p>The harbor ledger records every tide that reached the stone quay and the clerks copy each entry twice.</p></body></html>")
                .insert_header("Content-Type", "text/html"),
        )
        .mount(&mock_server)
        .await;

    let out_dir = tempfile::TempDir::new().expect("temp output dir");
    // Harness `cmd()` already sets DISABLE_ENTRY_GUARD_ENV=1 — one-layer
    // disarmed posture, production resolver/redirect layers live.
    let output = common::cmd()
        .arg("--url")
        .arg(mock_server.uri())
        .arg("--use-sitemap")
        .arg("--output")
        .arg(out_dir.path())
        .arg("--timeout-secs")
        .arg("10")
        .arg("--max-retries")
        .arg("0")
        .timeout(RUN_TIMEOUT)
        .output()
        .expect("spawn webfang");
    let stderr = normalize_captured(&String::from_utf8_lossy(&output.stderr));
    assert!(
        output.status.success() || output.status.code() == Some(2),
        "positive control must reach the listener and complete (exit 0 or 2 \
         for an empty-relevance sitemap is acceptable; a 69 SSRF rejection \
         would mean the disarmer broke): exit {:?}\nstderr: {stderr}",
        output.status.code()
    );
    assert!(
        !stderr.contains("SSRF detectado"),
        "disarmed entry layer must not reject the loopback listener; the \
         rejection in row1/row2 must come from the ARMED entry guard, not the \
         network: {stderr}"
    );
    let requests = mock_server
        .received_requests()
        .await
        .expect("wiremock request journal");
    assert!(
        !requests.is_empty(),
        "disarmed entry layer must let the discovery chain reach the listener"
    );
}
