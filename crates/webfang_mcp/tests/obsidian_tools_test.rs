//! Obsidian tools behavioral coverage (issue #450).
//!
//! End-to-end tests for the three Obsidian integration tools:
//! - `build_obsidian_uri`: happy path, shell-metacharacter neutralization,
//!   control-character rejection
//! - `detect_obsidian_vault`: explicit CLI vault path resolution
//! - `open_in_obsidian`: validation error path (never launches a real app)
//!
//! Run with: cargo nextest run -p webfang_mcp --features mcp --test obsidian_tools_test

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
// Harness helpers — local copies (each integration test binary is standalone;
// webfang_core's tests/common is NOT importable from webfang_mcp).
// ============================================================================

/// Start a test MCP server on a random port and return the base URL.
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

/// Build a JSON-RPC request body for MCP protocol.
fn mcp_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
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

/// Initialize an MCP session (initialize + notifications/initialized) and
/// return the session ID.
async fn init_session(client: &Client, base_url: &str) -> String {
    let init_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "obsidian-tools-test", "version": "1.0.0" }
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
// build_obsidian_uri
// ============================================================================

/// The happy path produces the exact `obsidian://open?vault=...&file=...` URI
/// with slashes preserved (Obsidian file paths are not percent-encoded).
#[tokio::test]
async fn test_build_obsidian_uri_happy_path() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "build_obsidian_uri",
        json!({ "vault_name": "MyVault", "file_path": "Inbox/note" }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "build_obsidian_uri should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(
        tool_text(&result),
        "obsidian://open?vault=MyVault&file=Inbox/note"
    );
}

/// Shell metacharacters (cmd.exe / POSIX) in vault or file values are
/// percent-encoded, never echoed raw — the value can never be interpreted by
/// a shell. `&` appears legitimately as the query separator, so the value is
/// isolated before asserting.
#[tokio::test]
async fn test_build_obsidian_uri_neutralizes_shell_metacharacters() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "build_obsidian_uri",
        json!({ "vault_name": "foo|calc.exe", "file_path": "notes;drop" }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "metacharacters must be encoded, not rejected: {}",
        tool_text(&result)
    );
    let uri = tool_text(&result);

    // Isolate the vault and file values (between the query separators).
    let vault_value = uri
        .strip_prefix("obsidian://open?vault=")
        .and_then(|rest| rest.split('&').next())
        .unwrap_or_default();
    let file_value = uri.split("file=").nth(1).unwrap_or_default();

    for metachar in ['|', ';', '>', '<', '^', '(', ')', ' ', '"'] {
        assert!(
            !vault_value.contains(metachar),
            "metachar {metachar:?} leaked into vault value: {vault_value}"
        );
        assert!(
            !file_value.contains(metachar),
            "metachar {metachar:?} leaked into file value: {file_value}"
        );
    }
    assert!(
        uri.contains("vault=foo%7Ccalc.exe"),
        "pipe must be percent-encoded, got: {uri}"
    );
    assert!(
        uri.contains("file=notes%3Bdrop"),
        "semicolon must be percent-encoded, got: {uri}"
    );
}

/// Control characters have no legitimate place in a vault name and are
/// rejected outright with an honest Spanish `CallToolResult::error`.
#[tokio::test]
async fn test_build_obsidian_uri_rejects_control_chars() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "build_obsidian_uri",
        json!({ "vault_name": "My\nVault", "file_path": "note" }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        is_tool_error(&result),
        "control chars must yield isError:true, got: {}",
        tool_text(&result)
    );
    let text = tool_text(&result);
    assert!(
        text.contains("caracteres de control no permitidos"),
        "Spanish control-char error expected, got: {text}"
    );
}

// ============================================================================
// detect_obsidian_vault
// ============================================================================

/// An explicit CLI vault path (priority 1) that is a real vault (contains a
/// `.obsidian/` marker) is returned verbatim. The path is deterministic: the
/// detector returns at priority 1 and never touches env vars, the Obsidian
/// registry, or the real home directory.
#[tokio::test]
async fn test_detect_obsidian_vault_explicit_path() {
    let vault = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(vault.path().join(".obsidian")).unwrap();

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_obsidian_vault",
        json!({ "vault_path": vault.path().to_string_lossy() }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        !is_tool_error(&result),
        "detect_obsidian_vault should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(
        tool_text(&result),
        vault.path().to_string_lossy().to_string()
    );
}

// ============================================================================
// open_in_obsidian
// ============================================================================

/// `open_in_obsidian` must reject invalid input BEFORE attempting to launch
/// the Obsidian app. The control-character path is deterministic and never
/// spawns a real process (no `xdg-open` on CI).
#[tokio::test]
async fn test_open_in_obsidian_control_chars_validation_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "open_in_obsidian",
        json!({ "vault_name": "MyVault", "file_path": "note\rpath" }),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        is_tool_error(&result),
        "control chars must yield isError:true, got: {}",
        tool_text(&result)
    );
    let text = tool_text(&result);
    assert!(
        text.contains("caracteres de control no permitidos"),
        "Spanish control-char error expected, got: {text}"
    );
    assert!(
        !text.contains("Opened in Obsidian"),
        "no real app launch may be reported, got: {text}"
    );
}
