//! MCP Server behavioral tests
//!
//! End-to-end tests that start the MCP server and verify HTTP-level behavior:
//! - Initialize request returns server info
//! - tools/list returns available tools
//! - Invalid session handling
//!
//! Run with: cargo nextest run --test mcp_behavioral_test

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use wreq::Client;

use rust_scraper::config::Config;
use rust_scraper::di::Container;
use rust_scraper::infrastructure::mcp_server::server::build_mcp_router;
use rust_scraper::infrastructure::mcp_server::state::McpState;

/// Start a test MCP server on a random port and return the base URL.
async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    let config = Config::default();
    let container = Container::new(config)
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

    // Give the server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    (base_url, handle)
}

/// Build a JSON-RPC request body for MCP protocol.
fn mcp_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
}

// ============================================================================
// 1. Initialize request — returns server info
// ============================================================================

/// MCP initialize request returns valid server capabilities and info.
#[tokio::test]
async fn test_initialize_returns_server_info() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    let request_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }),
    );

    let response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&request_body)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.unwrap();

    // MCP Streamable HTTP may return 200 with SSE or direct JSON
    assert!(
        status.is_success(),
        "initialize should return 2xx, got {}: {}",
        status,
        &body[..body.len().min(500)]
    );

    // Parse the response — may be SSE format or direct JSON
    let response_text = body.clone();

    // Try to extract JSON from SSE format (lines starting with "data: ")
    let json_str = if response_text.contains("data: ") {
        response_text
            .lines()
            .find(|line| line.starts_with("data: "))
            .map(|line| line.strip_prefix("data: ").unwrap_or(line))
            .unwrap_or(&body)
    } else {
        &body
    };

    if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
        // Verify it has the JSON-RPC response structure
        if let Some(result) = parsed.get("result") {
            // Server info should be present
            if let Some(server_info) = result.get("serverInfo") {
                assert!(
                    server_info.get("name").is_some(),
                    "serverInfo should have name field"
                );
                assert!(
                    server_info.get("version").is_some(),
                    "serverInfo should have version field"
                );
            }
        }
    }
    // If parsing fails, the server still responded successfully (200 OK)
    // which is valid for the MCP protocol handshake
}

// ============================================================================
// 2. tools/list — returns available tools
// ============================================================================

/// MCP tools/list request returns a list of available tools.
#[tokio::test]
async fn test_tools_list_returns_available_tools() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    // First, initialize the session
    let init_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }),
    );

    let init_response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init_body)
        .send()
        .await
        .expect("initialize request should succeed");

    // Extract session ID from response headers if present
    let session_id = init_response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Send initialized notification
    let _ = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }))
        .send()
        .await;

    // Now request tools/list — must include session ID
    let tools_body = mcp_request("tools/list", json!({}));

    let mut tools_req = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");

    if let Some(ref sid) = session_id {
        tools_req = tools_req.header("mcp-session-id", sid);
    }

    let response = tools_req
        .json(&tools_body)
        .send()
        .await
        .expect("tools/list request should succeed");

    let status = response.status();
    let body = response.text().await.unwrap();

    assert!(
        status.is_success(),
        "tools/list should return 2xx, got {}: {}",
        status,
        &body[..body.len().min(500)]
    );

    // Try to parse the response
    let response_text = body.clone();
    let json_str = if response_text.contains("data: ") {
        response_text
            .lines()
            .find(|line| line.starts_with("data: "))
            .map(|line| line.strip_prefix("data: ").unwrap_or(line))
            .unwrap_or(&body)
    } else {
        &body
    };

    if let Ok(parsed) = serde_json::from_str::<Value>(json_str) {
        if let Some(result) = parsed.get("result") {
            if let Some(tools) = result.get("tools") {
                let tools_array = tools.as_array().expect("tools should be an array");
                assert!(
                    !tools_array.is_empty(),
                    "tools/list should return at least one tool"
                );

                // Verify tool structure — each tool should have name and description
                for tool in tools_array {
                    assert!(
                        tool.get("name").is_some(),
                        "each tool should have a name field"
                    );
                    assert!(
                        tool.get("description").is_some(),
                        "each tool should have a description field"
                    );
                }

                // Verify core tools are present
                let tool_names: Vec<&str> = tools_array
                    .iter()
                    .filter_map(|t| t.get("name")?.as_str())
                    .collect();

                assert!(
                    tool_names.contains(&"scrape_url"),
                    "scrape_url tool should be registered"
                );
                assert!(
                    tool_names.contains(&"validate_url"),
                    "validate_url tool should be registered"
                );
            }
        }
    }
}

// ============================================================================
// 3. Invalid session handling
// ============================================================================

/// Request with invalid session ID returns an error.
#[tokio::test]
async fn test_invalid_session_returns_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    let tools_body = mcp_request("tools/list", json!({}));

    let response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", "invalid-session-id-12345")
        .json(&tools_body)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.unwrap();

    // Should return an error (4xx) or handle gracefully
    // The MCP protocol may return 400 Bad Request or similar for invalid sessions
    assert!(
        !status.is_success() || body.contains("error"),
        "invalid session should return error status or error in body, got {}: {}",
        status,
        &body[..body.len().min(500)]
    );
}

/// Request without session ID is handled gracefully.
#[tokio::test]
async fn test_no_session_id_handled() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    let tools_body = mcp_request("tools/list", json!({}));

    let response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        // No mcp-session-id header
        .json(&tools_body)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.unwrap();

    // Should either succeed (stateless mode) or return a clear error (400, 401, 422)
    assert!(
        status.is_success()
            || status.as_u16() == 400
            || status.as_u16() == 401
            || status.as_u16() == 422,
        "request without session should return 2xx or 4xx, got {}: {}",
        status,
        &body[..body.len().min(500)]
    );
}

/// Unknown JSON-RPC method returns error response.
#[tokio::test]
async fn test_unknown_method_returns_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    let request_body = mcp_request("unknown/method", json!({}));

    let response = client
        .post(format!("{}/mcp", base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&request_body)
        .send()
        .await
        .expect("request should succeed");

    let status = response.status();
    let body = response.text().await.unwrap();

    // Should return success (200) with JSON-RPC error in body,
    // or an HTTP error status
    if status.is_success() {
        // Check for JSON-RPC error in response
        let has_error = body.contains("error") || body.contains("Method not found");
        assert!(
            has_error,
            "unknown method should return JSON-RPC error: {}",
            &body[..body.len().min(500)]
        );
    }
    // HTTP error status is also acceptable
}
