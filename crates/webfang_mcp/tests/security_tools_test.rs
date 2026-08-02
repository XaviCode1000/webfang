//! Security & diagnostics tools behavioral coverage (issue #450).
//!
//! End-to-end tests for the WAF tools:
//! - `detect_waf`: degraded-mode challenge detection + the #346 false-positive
//!   guard (bare fingerprints never block without HTTP context)
//! - `verify_waf_integrity`: degraded-mode header semantics (#346) and the
//!   additive status/content_type context (T2 fingerprint blocks on a WAF
//!   status, passes on an OK status)
//! - `list_waf_providers`: real provider list
//!
//! `get_scrape_metrics` is covered by mcp_behavioral_test.rs.
//!
//! Run with: cargo nextest run -p webfang_mcp --features mcp --test security_tools_test

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use wreq::Client;

use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::state::McpState;

// ============================================================================
// Harness helpers — local copies (each integration test binary is standalone).
// ============================================================================

async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
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

    for _ in 0..20 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    (base_url, handle)
}

fn mcp_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
}

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

async fn init_session(client: &Client, base_url: &str) -> String {
    let init_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "security-tools-test", "version": "1.0.0" }
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

fn is_tool_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ============================================================================
// detect_waf — degraded mode (no HTTP context)
// ============================================================================

/// A Challenge-tier (T1) marker blocks even in degraded mode.
#[tokio::test]
async fn test_detect_waf_challenge_marker_is_detected() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_waf",
        json!({ "html": r#"<div id="cf-turnstile" data-sitekey="abc"></div>"# }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "detect_waf should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(tool_text(&result), "WAF detected: Cloudflare Turnstile");
}

/// A bare vendor fingerprint (T2) is evidence only and NEVER blocks in
/// degraded mode — the issue #346 false-positive fix.
#[tokio::test]
async fn test_detect_waf_bare_fingerprint_never_blocks_degraded() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_waf",
        json!({ "html": "<html><body>powered by cloudflare</body></html>" }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "detect_waf should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(tool_text(&result), "no WAF detected");
}

/// A clean body reports no WAF.
#[tokio::test]
async fn test_detect_waf_clean_body_no_waf() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_waf",
        json!({ "html": "<html><body>normal content</body></html>" }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "detect_waf should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(tool_text(&result), "no WAF detected");
}

// ============================================================================
// verify_waf_integrity — degraded mode + additive context
// ============================================================================

/// Degraded mode (no status/content_type): a control header (T2 fingerprint)
/// alone never blocks on mere presence — evidence is collected but the check
/// passes. This pins the issue #346 verdict change end-to-end.
#[tokio::test]
async fn test_verify_waf_integrity_header_alone_passes_degraded() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "verify_waf_integrity",
        json!({
            "html": "<html>clean</html>",
            "headers": { "x-datadome-response": "1" }
        }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "verify_waf_integrity should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(
        tool_text(&result),
        "WAF integrity check passed",
        "T2 header alone must not block in degraded mode (#346)"
    );
}

/// A Challenge-tier (T1) marker blocks even without HTTP context.
#[tokio::test]
async fn test_verify_waf_integrity_t1_challenge_blocks_degraded() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "verify_waf_integrity",
        json!({ "html": "Just a moment..." }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "verify_waf_integrity should succeed: {}",
        tool_text(&result)
    );
    assert!(
        tool_text(&result).starts_with("WAF blocked:"),
        "T1 challenge must block, got: {}",
        tool_text(&result)
    );
}

/// Additive context: a bare vendor fingerprint (T2) blocks when a correlated
/// WAF status (403) is supplied.
#[tokio::test]
async fn test_verify_waf_integrity_t2_with_waf_status_blocks() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "verify_waf_integrity",
        json!({
            "html": "<html>blocked by akamai</html>",
            "status": 403,
            "content_type": "text/html"
        }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "verify_waf_integrity should succeed: {}",
        tool_text(&result)
    );
    assert!(
        tool_text(&result).starts_with("WAF blocked:"),
        "T2 fingerprint + WAF status 403 must block, got: {}",
        tool_text(&result)
    );
}

/// Additive context: the SAME T2 body at an OK status (200) passes.
#[tokio::test]
async fn test_verify_waf_integrity_t2_with_ok_status_passes() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "verify_waf_integrity",
        json!({
            "html": "<html>blocked by akamai</html>",
            "status": 200,
            "content_type": "text/html"
        }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "verify_waf_integrity should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(
        tool_text(&result),
        "WAF integrity check passed",
        "T2 fingerprint at status 200 must pass, got: {}",
        tool_text(&result)
    );
}

// ============================================================================
// list_waf_providers
// ============================================================================

/// The provider list is real and non-empty, and includes known providers.
#[tokio::test]
async fn test_list_waf_providers_is_non_empty() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "list_waf_providers",
        json!({}),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "list_waf_providers should succeed: {}",
        tool_text(&result)
    );
    let text = tool_text(&result);
    assert!(!text.trim().is_empty(), "provider list must not be empty");
    for provider in ["Cloudflare", "DataDome", "Akamai"] {
        assert!(
            text.contains(provider),
            "provider list must contain {provider}, got: {text}"
        );
    }
}
