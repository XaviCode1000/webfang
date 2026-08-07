//! End-to-end MCP parameter-validation rejection tests (issue #512, slice 2).
//!
//! Every test starts the real MCP server and invokes a tool over HTTP, then
//! asserts on the JSON-RPC error *code* (never on message strings). Most
//! handlers wire `params.validate()?` as the first statement, so invalid
//! parameters are rejected at the protocol boundary with code `-32602`
//! (invalid params) before any network access or semaphore acquisition.
//! Exceptions: `validate_url` (returns tool-level `{"valid": false}`) and
//! `detect_obsidian_vault` (accepts absolute paths) — see bug #590.
//!
//! Run with: cargo nextest run --test params_rejection_test --features mcp

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use wreq::Client;

use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::state::McpState;

/// JSON-RPC standard error code for "Invalid params" (JSON-RPC 2.0 spec).
const JSONRPC_INVALID_PARAMS: i64 = -32602;

/// `validation::MAX_BLOB_LEN` (1_048_576) + 1, to exceed the max blob length.
const MAX_BLOB_LEN_PLUS_1: usize = 1_048_577;

// ============================================================================
// Harness (copied from mcp_behavioral_test.rs — each integration test binary
// is standalone and cannot import another binary's helpers).
// ============================================================================

/// Start a test MCP server on a random port and return the base URL.
async fn start_test_server() -> (String, JoinHandle<()>) {
    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let app = build_mcp_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Wait for the server to accept TCP connections instead of a fixed sleep.
    for _ in 0..20 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    (base_url, handle)
}

/// Build a JSON-RPC request body for the MCP protocol.
fn mcp_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
}

/// Initialize an MCP session (initialize + notifications/initialized) and
/// return the session ID.
async fn init_session(client: &Client, base_url: &str) -> String {
    let init_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "rejection-test", "version": "1.0.0" }
        }),
    );
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init_body)
        .send()
        .await
        .expect("initialize should succeed");
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("initialize must return mcp-session-id");

    let _ = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await;

    session_id
}

/// Call an MCP tool and return the parsed JSON-RPC response object.
async fn call_tool(
    client: &Client,
    base_url: &str,
    session_id: &str,
    name: &str,
    args: Value,
) -> Value {
    let body = mcp_request("tools/call", json!({ "name": name, "arguments": args }));
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id)
        .json(&body)
        .send()
        .await
        .expect("tools/call should succeed");
    let text = resp.text().await.expect("read response body");
    extract_json(&text).expect("response must parse as JSON-RPC")
}

/// Extract the first JSON-RPC object from an SSE (`data: ` prefixed) or direct
/// JSON response body.
fn extract_json(body: &str) -> Option<Value> {
    if body.contains("data: ") {
        body.lines()
            .filter(|line| line.starts_with("data: "))
            .filter_map(|line| {
                let json_str = line.strip_prefix("data: ").unwrap_or(line);
                serde_json::from_str::<Value>(json_str).ok()
            })
            .next()
    } else {
        serde_json::from_str::<Value>(body).ok()
    }
}

/// Extract the JSON-RPC error code from a parsed response, if present.
fn error_code(result: &Value) -> Option<i64> {
    result
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
}

/// Extract the first content text from a tool result object.
fn tool_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Whether a tool result is flagged as an error (CallToolResult::error).
fn is_tool_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ============================================================================
// Rejection tests — invalid parameters must map to JSON-RPC -32602 and never
// reach network/IO.
// ============================================================================

/// `scrape_url` rejects a `file://` URL (unsupported scheme).
#[tokio::test]
async fn scrape_url_rejects_file_scheme() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "file:///etc/passwd" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "file:// scheme must be rejected with -32602, got: {resp}"
    );
}

/// `scrape_url` rejects an `ftp://` URL (unsupported scheme).
#[tokio::test]
async fn scrape_url_rejects_ftp_scheme() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "ftp://example.com/file" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "ftp:// scheme must be rejected with -32602, got: {resp}"
    );
}

/// `validate_url` returns a tool-level result (not a protocol error) for a
/// `file://` URL (bug #7 fix: tool reports parsed result instead of rejecting).
#[tokio::test]
async fn validate_url_file_scheme_returns_tool_result() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "validate_url",
        json!({ "url": "file:///etc/passwd" }),
    )
    .await;

    // After bug #7 fix: validate_url no longer calls params.validate()?.
    // It returns a JSON tool result for ALL inputs (file:// is a valid URL per
    // RFC 3986, so it reports valid:true with scheme:file).
    assert_eq!(
        error_code(&resp),
        None,
        "validate_url must NOT return protocol error, got: {resp}"
    );
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected a tool result, got: {resp}"));
    assert!(
        !is_tool_error(result),
        "validate_url file:// must not be a tool error"
    );
}

/// `extract_domain` rejects a non-http(s) URL.
#[tokio::test]
async fn extract_domain_rejects_file_scheme() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "extract_domain",
        json!({ "url": "file:///etc/passwd" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "extract_domain file:// scheme must be rejected with -32602, got: {resp}"
    );
}

/// `crawl_site` rejects an unsupported scheme.
#[tokio::test]
async fn crawl_site_rejects_ftp_scheme() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "crawl_site",
        json!({ "url": "ftp://example.com" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "crawl_site ftp:// scheme must be rejected with -32602, got: {resp}"
    );
}

/// `crawl_site` rejects a `max_depth` beyond the allowed bound (> 10).
#[tokio::test]
async fn crawl_site_rejects_max_depth_beyond_limit() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "crawl_site",
        json!({ "url": "https://example.com", "max_depth": 11 }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "max_depth > 10 must be rejected with -32602, got: {resp}"
    );
}

/// `scrape_with_options` rejects an unknown field (deny_unknown_fields) at the
/// deserialization boundary, mapped to -32602.
#[tokio::test]
async fn scrape_with_options_rejects_unknown_field() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_with_options",
        json!({ "url": "https://example.com", "typo_field": 1 }),
    )
    .await;

    // rmcp 1.8.0 surfaces deserialization failures (deny_unknown_fields) as a
    // tool-level error (isError:true), not a JSON-RPC -32602 protocol error.
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected a tool result, got: {resp}"));
    assert!(
        is_tool_error(result),
        "unknown field must be rejected (deny_unknown_fields), got: {}",
        tool_text(result)
    );
}

/// `export_file` rejects a path-traversal `output_dir`.
#[tokio::test]
async fn export_file_rejects_output_dir_traversal() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_file",
        json!({
            "output_dir": "../escape",
            "filename": "out",
            "format": "jsonl",
            "content": "hello"
        }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "output_dir traversal must be rejected with -32602, got: {resp}"
    );
}

/// `export_file` rejects an absolute `output_dir`.
#[tokio::test]
async fn export_file_rejects_absolute_output_dir() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_file",
        json!({
            "output_dir": "/tmp/webfang-export",
            "filename": "out",
            "format": "jsonl",
            "content": "hello"
        }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "absolute output_dir must be rejected with -32602, got: {resp}"
    );
}

/// `export_file` rejects an unrecognized export format.
#[tokio::test]
async fn export_file_rejects_unknown_format() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_file",
        json!({
            "output_dir": "exports",
            "filename": "out",
            "format": "bogus",
            "content": "hello"
        }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "unknown format must be rejected with -32602, got: {resp}"
    );
}

/// `download_assets` rejects a path-traversal `output_dir`.
#[tokio::test]
async fn download_assets_rejects_output_dir_traversal() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "download_assets",
        json!({
            "html": "<img src='https://example.com/a.png'>",
            "base_url": "https://example.com",
            "images": true,
            "documents": false,
            "output_dir": "../escape"
        }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "download_assets output_dir traversal must be rejected with -32602, got: {resp}"
    );
}

/// `detect_obsidian_vault` accepts an absolute `vault_path` (bug #8 fix).
#[tokio::test]
async fn detect_obsidian_vault_accepts_absolute_path() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_obsidian_vault",
        json!({ "vault_path": "/tmp/some-vault" }),
    )
    .await;

    // After bug #8 fix: absolute paths are accepted (no -32602).
    // The tool will return a result (vault not found, but not a validation error).
    assert_eq!(
        error_code(&resp),
        None,
        "absolute vault_path must NOT be rejected with -32602, got: {resp}"
    );
}

/// `build_obsidian_uri` rejects a path-traversal `file_path`.
#[tokio::test]
async fn build_obsidian_uri_rejects_traversal_file_path() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "build_obsidian_uri",
        json!({ "vault_name": "MyVault", "file_path": "../escape" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "file_path traversal must be rejected with -32602, got: {resp}"
    );
}

/// `clean_html` rejects an oversize HTML blob.
#[tokio::test]
async fn clean_html_rejects_oversize_blob() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "clean_html",
        json!({ "html": "a".repeat(MAX_BLOB_LEN_PLUS_1) }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "oversize html blob must be rejected with -32602, got: {resp}"
    );
}

/// `detect_waf` rejects an oversize HTML blob (no network access).
#[tokio::test]
async fn detect_waf_rejects_oversize_html() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_waf",
        json!({ "html": "a".repeat(MAX_BLOB_LEN_PLUS_1) }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "oversize html blob must be rejected with -32602, got: {resp}"
    );
}

// ============================================================================
// Control tests — valid parameters must NOT be rejected (-32602 absent) and
// must reach the tool body (proving validation does not over-reject).
// ============================================================================

/// `validate_url` accepts a valid https URL and succeeds (no -32602).
#[tokio::test]
async fn validate_url_accepts_https() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "validate_url",
        json!({ "url": "https://example.com/path?q=1" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        None,
        "valid https URL must not be rejected, got: {resp}"
    );
    assert!(
        !is_tool_error(resp.get("result").unwrap_or(&Value::Null)),
        "valid https URL result must not be an error: {}",
        tool_text(resp.get("result").unwrap_or(&Value::Null))
    );
}

/// `extract_links` accepts valid html + base_url and succeeds (no -32602).
#[tokio::test]
async fn extract_links_accepts_valid() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "extract_links",
        json!({
            "html": "<html><body><a href=\"/page\">link</a></body></html>",
            "base_url": "https://example.com"
        }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        None,
        "valid extract_links params must not be rejected, got: {resp}"
    );
}

/// `convert_wiki_links` accepts a bare `base_domain` (the documented format)
/// and must NOT be rejected by validation.
#[tokio::test]
async fn convert_wiki_links_accepts_bare_domain() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "convert_wiki_links",
        json!({ "markdown": "[link](/page)", "base_domain": "example.com" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        None,
        "bare base_domain must not be rejected, got: {resp}"
    );
    assert!(
        !is_tool_error(resp.get("result").unwrap_or(&Value::Null)),
        "bare base_domain result must not be an error: {}",
        tool_text(resp.get("result").unwrap_or(&Value::Null))
    );
}

/// `is_internal_link` accepts a full-URL `seed_domain` (the core's
/// `normalize_seed_host` handles it) and must NOT be rejected by validation.
#[tokio::test]
async fn is_internal_link_accepts_full_url_seed() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "is_internal_link",
        json!({ "url": "https://example.com/page", "seed_domain": "https://example.com" }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        None,
        "full-URL seed_domain must not be rejected, got: {resp}"
    );
    assert!(
        !is_tool_error(resp.get("result").unwrap_or(&Value::Null)),
        "full-URL seed_domain result must not be an error: {}",
        tool_text(resp.get("result").unwrap_or(&Value::Null))
    );
}
