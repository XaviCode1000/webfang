//! MCP Server lifecycle test — init → notify → list → call
//!
//! Tests the full MCP session lifecycle over HTTP Streamable transport.
//! This catches session management bugs where state is lost between requests.
//!
//! Run: cargo test --test mcp_lifecycle_test -- --nocapture

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use wreq::Client;

use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::state::McpState;

/// Start a test MCP server on a random port and return the base URL.
///
/// NOTE: Container::new creates real HTTP clients (wreq) and a real service layer.
/// This is intentional for integration tests — the container is ephemeral and
/// scoped to the test, so real infrastructure gives us confidence that the MCP
/// server works end-to-end with the actual application state.
async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let app = build_mcp_router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);

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

/// Helper: extract JSON from SSE or direct JSON response.
///
/// NOTE: This reimplements SSE parsing inline. If more MCP test files are added,
/// consider extracting this to `tests/common/mod.rs` to avoid duplication.
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

// ============================================================================
// Full lifecycle: init → notify → list → call
// ============================================================================

/// Test the complete MCP lifecycle: initialize → notifications/initialized →
/// tools/list → tools/call, verifying session state persists across all requests.
#[tokio::test]
async fn test_full_lifecycle_init_notify_list_call() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    // ── Step 1: initialize ────────────────────────────────────────────
    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "lifecycle-test",
                "version": "1.0.0"
            }
        }
    });

    let init_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init_body)
        .send()
        .await
        .expect("initialize request should succeed");

    let init_status = init_response.status();
    assert!(
        init_status.is_success(),
        "initialize should return 2xx, got {}",
        init_status
    );

    // Extract session ID from response headers
    let session_id = init_response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("initialize response must include mcp-session-id header");

    println!("Step 1: session_id = {}", session_id);
    assert!(!session_id.is_empty(), "session ID must not be empty");

    // Parse initialize response
    let init_body = init_response.text().await.unwrap();
    let init_parsed = extract_json(&init_body);
    if let Some(parsed) = &init_parsed {
        if let Some(result) = parsed.get("result") {
            if let Some(server_info) = result.get("serverInfo") {
                println!(
                    "  server: {} v{}",
                    server_info.get("name").unwrap(),
                    server_info.get("version").unwrap()
                );
            }
        }
    }

    // ── Step 2: notifications/initialized (WITH session ID) ───────────
    let notify_body = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });

    let notify_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&notify_body)
        .send()
        .await
        .expect("notifications/initialized should succeed");

    let notify_status = notify_response.status();
    println!(
        "Step 2: notifications/initialized → status {}",
        notify_status
    );
    // 202 Accepted is the expected response for notifications
    assert!(
        notify_status.is_success() || notify_status.as_u16() == 202,
        "notifications/initialized should return 2xx or 202, got {}",
        notify_status
    );

    // ── Step 3: tools/list (WITH session ID) ──────────────────────────
    let list_body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    let list_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&list_body)
        .send()
        .await
        .expect("tools/list request should succeed");

    let list_status = list_response.status();
    let list_body = list_response.text().await.unwrap();
    println!("Step 3: tools/list → status {}", list_status);

    assert!(
        list_status.is_success(),
        "tools/list should return 2xx, got {}: {}",
        list_status,
        &list_body[..list_body.len().min(500)]
    );

    let list_parsed = extract_json(&list_body);
    let tools = list_parsed
        .and_then(|v| v.get("result")?.get("tools")?.as_array().cloned())
        .unwrap_or_default();

    assert!(
        !tools.is_empty(),
        "tools/list should return at least one tool, body: {}",
        &list_body[..list_body.len().min(500)]
    );

    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name")?.as_str())
        .collect();

    assert!(
        tool_names.contains(&"validate_url"),
        "validate_url should be registered"
    );
    assert!(
        tool_names.contains(&"clean_html"),
        "clean_html should be registered"
    );
    println!("  found {} tools", tool_names.len());

    // ── Step 4: tools/call validate_url (WITH session ID) ─────────────
    let call_body = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "validate_url",
            "arguments": {
                "url": "https://example.com:8080/path?q=1"
            }
        }
    });

    let call_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&call_body)
        .send()
        .await
        .expect("tools/call request should succeed");

    let call_status = call_response.status();
    let call_body = call_response.text().await.unwrap();
    println!("Step 4: tools/call validate_url → status {}", call_status);

    assert!(
        call_status.is_success(),
        "tools/call should return 2xx, got {}: {}",
        call_status,
        &call_body[..call_body.len().min(500)]
    );

    let call_parsed = extract_json(&call_body);
    let call_text = call_parsed
        .and_then(|v| {
            v.get("result")?
                .get("content")?
                .as_array()?
                .first()?
                .get("text")?
                .as_str()
                .map(String::from)
        })
        .unwrap_or_default();

    assert!(
        call_text.contains("example.com"),
        "validate_url should return host info, got: {}",
        call_text
    );
    println!("  result: {}", &call_text[..call_text.len().min(200)]);

    // ── Step 5: tools/call clean_html (second call, same session) ─────
    let call2_body = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "clean_html",
            "arguments": {
                "html": "<html><script>alert('x')</script><body><p>Hello World</p></body></html>"
            }
        }
    });

    let call2_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&call2_body)
        .send()
        .await
        .expect("second tools/call should succeed");

    let call2_status = call2_response.status();
    let call2_body = call2_response.text().await.unwrap();
    println!("Step 5: tools/call clean_html → status {}", call2_status);

    assert!(
        call2_status.is_success(),
        "second tools/call should return 2xx, got {}: {}",
        call2_status,
        &call2_body[..call2_body.len().min(500)]
    );

    let call2_parsed = extract_json(&call2_body);
    let call2_text = call2_parsed
        .and_then(|v| {
            v.get("result")?
                .get("content")?
                .as_array()?
                .first()?
                .get("text")?
                .as_str()
                .map(String::from)
        })
        .unwrap_or_default();

    assert!(
        !call2_text.contains("<script>"),
        "clean_html should remove scripts, got: {}",
        call2_text
    );
    assert!(
        call2_text.contains("Hello World"),
        "clean_html should preserve content, got: {}",
        call2_text
    );

    println!("\n✓ Full lifecycle test passed: init → notify → list → call (x2)");
}

// ============================================================================
// Edge case: missing initialized notification
// ============================================================================

/// Test that tools/list works even if notifications/initialized is not sent.
/// The session should still be usable after just the initialize handshake.
#[tokio::test]
async fn test_session_works_without_initialized_notification() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    // Step 1: initialize
    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1.0.0" }
        }
    });

    let init_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init_body)
        .send()
        .await
        .expect("initialize should succeed");

    let session_id = init_response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("must have session ID");

    // Skip notifications/initialized entirely

    // Step 2: tools/list directly
    let list_body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });

    let list_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&list_body)
        .send()
        .await
        .expect("tools/list should succeed");

    let list_status = list_response.status();
    let list_body = list_response.text().await.unwrap();

    assert!(
        list_status.is_success(),
        "tools/list should work without initialized notification, got {}: {}",
        list_status,
        &list_body[..list_body.len().min(500)]
    );

    println!("✓ Session works without initialized notification");
}

// ============================================================================
// Edge case: no session ID on non-init request
// ============================================================================

/// Verify that sending a non-initialize request without session ID
/// returns 422 (not a crash or 500).
#[tokio::test]
async fn test_no_session_id_returns_422() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    let response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&body)
        .send()
        .await
        .expect("request should not crash");

    let status = response.status();
    let body = response.text().await.unwrap();

    assert_eq!(
        status.as_u16(),
        422,
        "non-init request without session ID should return 422, got {}: {}",
        status,
        &body[..body.len().min(500)]
    );
    assert!(
        body.contains("initialize"),
        "error message should mention initialize, got: {}",
        body
    );
}
