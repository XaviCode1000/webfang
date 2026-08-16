//! SSRF integration probe — the guard tested LIVE with protection ENABLED.
//!
//! This is the ONLY MCP integration suite that runs with the SSRF guard ON
//! (via `start_test_server_ssrf_enabled()`). Every other suite disables it
//! through `WEBFANG_MCP_DISABLE_SSRF=1` because wiremock binds 127.0.0.1.
//!
//! Proves #703 Paso 1's "live criterion": IPv4-mapped IPv6 loopback,
//! cloud-metadata, and CGNAT targets are rejected at the PROTOCOL level —
//! JSON-RPC `error.code == -32602` (invalid params) — BEFORE any fetch is
//! attempted. That is, a forbidden target can never surface as a tool result
//! (`isError:true`) with the HTTP fetch already performed.
//!
//! No mock server, no network: the guard resolves the target host and fails
//! during DNS validation. IP literals are resolved locally by
//! `tokio::net::lookup_host`, so no listener is needed on the (closed)
//! discarded port and the behavior is fully deterministic.

#![cfg(feature = "mcp")]

mod common;
use common::*;

use serde_json::json;
use wreq::Client;

/// JSON-RPC standard error code for "Invalid params" (JSON-RPC 2.0 spec) —
/// the protocol-level code `McpError::invalid_params` maps to.
const JSONRPC_INVALID_PARAMS: i64 = -32602;

/// Exact Spanish prefix of the SSRF rejection message built in
/// `src/mcp_server/ssrf.rs` ("SSRF detectado: IP {ip} prohibida ...").
const SSRF_ERROR_PREFIX: &str = "SSRF detectado";

/// Assert that `resp` is a JSON-RPC protocol error with code -32602 whose
/// message carries the SSRF rejection text (same assertion style as
/// `test_export_invalid_format_hard_error` in `mcp_behavioral_test.rs`).
fn assert_ssrf_rejection(resp: serde_json::Value, url: &str) {
    let error = resp
        .get("error")
        .unwrap_or_else(|| panic!("scrape_url({url}) must return a JSON-RPC error, got: {resp}"));
    let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    assert_eq!(
        code, JSONRPC_INVALID_PARAMS,
        "scrape_url({url}) must map to JSON-RPC invalid-params (-32602), got: {resp}"
    );
    let message = error.get("message").and_then(|m| m.as_str()).unwrap_or("");
    assert!(
        message.contains(SSRF_ERROR_PREFIX),
        "scrape_url({url}) error message must contain {SSRF_ERROR_PREFIX:?}, got: {resp}"
    );
}

/// IPv4-mapped IPv6 loopback (`::ffff:127.0.0.1`) must be caught by the guard
/// before any fetch — a plain protocol error, never a tool error.
#[tokio::test]
async fn mapped_loopback_probe_returns_32602() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "http://[::ffff:127.0.0.1]:9/" }),
    )
    .await;

    assert_ssrf_rejection(resp, "http://[::ffff:127.0.0.1]:9/");
}

/// IPv4-mapped IPv6 cloud-metadata endpoint (`::ffff:169.254.169.254`) — the
/// canonical SSRF target — must be caught before any fetch.
#[tokio::test]
async fn mapped_metadata_probe_returns_32602() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "http://[::ffff:169.254.169.254]:9/" }),
    )
    .await;

    assert_ssrf_rejection(resp, "http://[::ffff:169.254.169.254]:9/");
}

/// IPv4-mapped IPv6 CGNAT address (`::ffff:100.64.0.1`) must be caught before
/// any fetch.
#[tokio::test]
async fn mapped_cgnat_probe_returns_32602() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "http://[::ffff:100.64.0.1]:9/" }),
    )
    .await;

    assert_ssrf_rejection(resp, "http://[::ffff:100.64.0.1]:9/");
}

/// Negative control: plain IPv4 loopback (`127.0.0.1`) is also rejected, which
/// proves the guard is genuinely ENABLED in this binary. If the disable flag
/// had leaked, this would fail with a connection-refused TOOL error instead of
/// a protocol-level `-32602`.
#[tokio::test]
async fn plain_loopback_probe_returns_32602() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "http://127.0.0.1:9/" }),
    )
    .await;

    assert_ssrf_rejection(resp, "http://127.0.0.1:9/");
}

/// REQ-02 (#707): `crawl_with_sitemap` validates the explicit `sitemap_url`
/// at entry with the guard ON — a cloud-metadata sitemap is rejected with the
/// protocol-level `-32602` BEFORE any fetch. The seed uses a public IP so its
/// own validation resolves locally (no DNS/network beyond fail-before-fetch);
/// port 9 (discard) is closed, and the assertion proves nothing was reached.
#[tokio::test]
async fn crawl_with_sitemap_metadata_sitemap_url_returns_32602() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "crawl_with_sitemap",
        json!({
            "url": "http://8.8.8.8/",
            "sitemap_url": "http://169.254.169.254/sitemap.xml",
        }),
    )
    .await;

    assert_ssrf_rejection(
        resp,
        "crawl_with_sitemap(sitemap_url=http://169.254.169.254/sitemap.xml)",
    );
}
