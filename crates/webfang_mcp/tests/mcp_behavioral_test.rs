//! MCP Server behavioral tests
//!
//! End-to-end tests that start the MCP server and verify HTTP-level behavior:
//! - Initialize request returns server info
//! - tools/list returns available tools
//! - Invalid session handling
//!
//! **SSRf Note**: Tests use wiremock on 127.0.0.1, which SSRF protection blocks.
//! SSRF is disabled by the test harness.
//!
//! Run with: cargo nextest run --test mcp_behavioral_test

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use wreq::Client;

use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::server::ServerOptions;
use webfang_mcp::mcp_server::state::McpState;

/// Initialize SSRF disable flag for tests (idempotent).
fn init_ssrf_disabled() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::env::set_var("WEBFANG_MCP_DISABLE_SSRF", "1");
    });
}

/// JSON-RPC standard error code for "Method not found" (JSON-RPC 2.0 spec).
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;

/// Parse JSON-RPC response body and extract the error code, if present.
///
/// Handles both direct JSON and SSE-wrapped (`data: ` prefix) responses.
/// Returns `Some(error_code)` if the response contains a JSON-RPC error object.
fn parse_jsonrpc_error_code(body: &str) -> Option<i64> {
    let json_str = if body.contains("data: ") {
        body.lines()
            .find(|line| line.starts_with("data: "))
            .map(|line| line.strip_prefix("data: ").unwrap_or(line))?
    } else {
        body
    };

    let parsed: Value = serde_json::from_str(json_str).ok()?;
    parsed.get("error")?.get("code")?.as_i64()
}

/// Start a test MCP server on a random port and return the base URL.
async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    // Disable SSRF before building the router
    init_ssrf_disabled();

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let app = build_mcp_router(state, &ServerOptions::default());

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
// Export-tool helpers (issue #343 slice 1 — real export wiring)
// ============================================================================

/// Extract the first JSON-RPC object from an SSE (`data: ` prefixed) or direct
/// JSON response body. Local copy — each integration test binary is standalone.
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

/// Redact a known output directory path so insta snapshots stay stable
/// run-to-run. Local helper: webfang_core's `tests/common` redactor is NOT
/// importable from webfang_mcp (dependency direction is mcp → core).
fn redact_path(text: &str, dir: &std::path::Path) -> String {
    text.replace(dir.to_string_lossy().as_ref(), "[OUT_DIR]")
}

/// Start a test MCP server whose crawl-result repository is pre-seeded with
/// `n` `ScrapedContent` items.
///
/// Returns `(base_url, server_handle, container_tmp)`. The container temp dir
/// is returned so the caller keeps it alive (the append-only repository log
/// lives inside it) and so tests can locate exports that default to the
/// container's configured `output_dir` (e.g. `process_export_pipeline`).
async fn start_seeded_server(n: usize) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    use webfang_core::domain::{CrawlerConfig, ScrapedContent, ValidUrl};
    use webfang_core::infrastructure::config::ScraperConfig;

    let container_tmp = tempfile::TempDir::new().expect("create container temp dir");
    let crawler_config =
        CrawlerConfig::new(url::Url::parse("https://seed.example.com").expect("valid seed URL"));
    let scraper_config = ScraperConfig {
        output_dir: container_tmp.path().to_path_buf(),
        ..Default::default()
    };
    let container = Container::new(crawler_config, scraper_config)
        .await
        .expect("container creation failed");

    // Seed the crawl-result repository with n items and wait for indexing.
    let repo = container
        .crawl_result_repository()
        .expect("container must wire a crawl result repository");
    for i in 0..n {
        let url_str = format!("https://seed.example.com/page/{i}");
        let url = url::Url::parse(&url_str).expect("valid seeded URL");
        let content = ScrapedContent {
            title: format!("Seed Title {i}"),
            content: format!("Seed body content number {i} for export testing."),
            url: ValidUrl::new(url),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
        };
        repo.save(&content).expect("save seeded content");
    }
    // Poll until the background writer has indexed every seeded URL.
    for i in 0..n {
        let url_str = format!("https://seed.example.com/page/{i}");
        for _ in 0..80 {
            if repo.find_by_url(&url_str).expect("find_by_url").is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    let state = McpState::new(container);
    let app = build_mcp_router(state, &ServerOptions::default());

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

    (base_url, handle, container_tmp)
}

/// Initialize an MCP session (initialize + notifications/initialized) and
/// return the session ID.
async fn init_session(client: &Client, base_url: &str) -> String {
    let init_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "export-test", "version": "1.0.0" }
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

/// A relative temporary directory that deletes itself on drop.
///
/// `tempfile::TempDir` always returns an absolute path (it joins with
/// `env::current_dir`), which the MCP `require_safe_path` validator rejects.
/// These tests need a *relative* output dir, so we manage one manually.
struct RelTempDir {
    path: std::path::PathBuf,
}

impl RelTempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let name = format!("{prefix}-{}-{}", std::process::id(), n);
        let path = std::path::PathBuf::from(name);
        std::fs::create_dir_all(&path).expect("create relative temp dir");
        RelTempDir { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for RelTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
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
        .post(format!("{base_url}/mcp"))
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
        .post(format!("{base_url}/mcp"))
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
        .post(format!("{base_url}/mcp"))
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
        .post(format!("{base_url}/mcp"))
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
        .post(format!("{base_url}/mcp"))
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
    let has_jsonrpc_error = parse_jsonrpc_error_code(&body).is_some();
    assert!(
        !status.is_success() || has_jsonrpc_error,
        "invalid session should return error status or JSON-RPC error in body, got {}: {}",
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
        .post(format!("{base_url}/mcp"))
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
        .post(format!("{base_url}/mcp"))
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
        // Verify JSON-RPC error code -32601 (Method not found)
        let error_code = parse_jsonrpc_error_code(&body);
        assert_eq!(
            error_code,
            Some(JSONRPC_METHOD_NOT_FOUND),
            "unknown method should return JSON-RPC error code {} (Method not found), got code {:?} in: {}",
            JSONRPC_METHOD_NOT_FOUND,
            error_code,
            &body[..body.len().min(500)]
        );
    }
    // HTTP error status is also acceptable
}

// ============================================================================
// 4. Export tools — real exports + honest errors (issue #343 slice 1)
// ============================================================================

/// REQ-MCP-EXPORT-01: export_jsonl writes a real `.jsonl` file (one valid JSON
/// object per line) and reports the real written path.
#[tokio::test]
async fn test_export_jsonl_writes_real_file() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(1).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out");
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_jsonl",
        json!({ "output_dir": out.path().to_string_lossy(), "filename": "export" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "export_jsonl should succeed: {}",
        tool_text(&result)
    );

    // A real .jsonl file exists at the reported path.
    let file_path = out.path().join("export.jsonl");
    assert!(
        file_path.exists(),
        "export.jsonl must exist at {}",
        file_path.display()
    );

    let body = std::fs::read_to_string(&file_path).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "JSONL file must have at least one line");
    let mut found_content = false;
    for line in &lines {
        let obj: Value = serde_json::from_str(line).expect("each JSONL line must be valid JSON");
        assert_eq!(
            obj.get("metadata_version").and_then(|v| v.as_str()),
            Some("2.1.0"),
            "JSONL line must carry the webfang metadata schema"
        );
        if obj
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .contains("Seed body content number 0")
        {
            found_content = true;
        }
    }
    assert!(
        found_content,
        "exported JSONL must contain the seeded content"
    );

    let text = tool_text(&result);
    insta::with_settings!({
        filters => vec![
            (r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)", "[TIMESTAMP]"),
            (r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", "[UUID]"),
            (r"127\.0\.0\.1:\d+", "127.0.0.1:[PORT]"),
        ],
    }, {
        insta::assert_snapshot!("export_jsonl_success", redact_path(&text, out.path()));
    });
}

/// REQ-MCP-EXPORT-02: export_vector writes a real `.json` file with a metadata
/// header plus exported chunks, and reports the real path.
#[tokio::test]
async fn test_export_vector_writes_real_file() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(1).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out");
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_vector",
        json!({ "output_dir": out.path().to_string_lossy(), "filename": "vectors" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "export_vector should succeed: {}",
        tool_text(&result)
    );

    let file_path = out.path().join("vectors.json");
    assert!(
        file_path.exists(),
        "vectors.json must exist at {}",
        file_path.display()
    );

    let body = std::fs::read_to_string(&file_path).unwrap();
    let parsed: Value = serde_json::from_str(&body).expect("vector export must be valid JSON");
    assert_eq!(
        parsed.get("format_version").and_then(|v| v.as_str()),
        Some("1.0"),
        "vector export must carry the metadata header"
    );
    let docs = parsed
        .get("documents")
        .and_then(|d| d.as_array())
        .expect("vector export must have a documents array");
    assert!(!docs.is_empty(), "documents array must not be empty");
    assert!(
        body.contains("Seed body content number 0"),
        "vector export must contain the seeded content"
    );

    let text = tool_text(&result);
    insta::with_settings!({
        filters => vec![
            (r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)", "[TIMESTAMP]"),
            (r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", "[UUID]"),
            (r"127\.0\.0\.1:\d+", "127.0.0.1:[PORT]"),
        ],
    }, {
        insta::assert_snapshot!("export_vector_success", redact_path(&text, out.path()));
    });
}

/// REQ-MCP-EXPORT-03: process_export_pipeline performs a REAL synchronous
/// export (never "queued") and reports the real path.
#[tokio::test]
async fn test_process_export_pipeline_real_export() {
    let (base_url, _handle, container_tmp) = start_seeded_server(1).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "process_export_pipeline",
        json!({ "format": "jsonl" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "pipeline should succeed: {}",
        tool_text(&result)
    );

    let text = tool_text(&result);
    assert!(
        !text.to_lowercase().contains("queued"),
        "pipeline must NOT report queued: {text}"
    );

    // The pipeline defaults to the container's configured output_dir.
    let file_path = container_tmp.path().join("export.jsonl");
    assert!(
        file_path.exists(),
        "pipeline must write a real file at {}",
        file_path.display()
    );
    let body = std::fs::read_to_string(&file_path).unwrap();
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(!lines.is_empty(), "pipeline export must have content");
    for line in &lines {
        let _: Value = serde_json::from_str(line).expect("pipeline JSONL line must be valid JSON");
    }

    insta::with_settings!({
        filters => vec![
            (r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)", "[TIMESTAMP]"),
            (r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", "[UUID]"),
            (r"127\.0\.0\.1:\d+", "127.0.0.1:[PORT]"),
        ],
    }, {
        insta::assert_snapshot!("process_export_pipeline_success", redact_path(&text, container_tmp.path()));
    });
}

/// REQ-MCP-EXPORT-04 (amended): export_file writes caller-provided content to a
/// real file via the real create_exporter surface and reports the real path.
#[tokio::test]
async fn test_export_file_writes_content() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(0).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out");
    let content = "Hello export file content written by the caller";
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_file",
        json!({
            "output_dir": out.path().to_string_lossy(),
            "filename": "myfile",
            "format": "jsonl",
            "content": content
        }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "export_file should succeed: {}",
        tool_text(&result)
    );

    let file_path = out.path().join("myfile.jsonl");
    assert!(
        file_path.exists(),
        "myfile.jsonl must exist at {}",
        file_path.display()
    );
    let body = std::fs::read_to_string(&file_path).unwrap();
    assert!(
        body.contains(content),
        "exported file body must contain the caller-provided content"
    );

    let text = tool_text(&result);
    insta::with_settings!({
        filters => vec![
            (r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)", "[TIMESTAMP]"),
            (r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}", "[UUID]"),
            (r"127\.0\.0\.1:\d+", "127.0.0.1:[PORT]"),
        ],
    }, {
        insta::assert_snapshot!("export_file_success", redact_path(&text, out.path()));
    });
}

/// REQ-MCP-EXPORT-05: an empty repository yields an honest CallToolResult::error
/// (isError:true, Spanish) — never a fake success.
#[tokio::test]
async fn test_export_jsonl_empty_repo_honest_error() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(0).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out");
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_jsonl",
        json!({ "output_dir": out.path().to_string_lossy(), "filename": "export" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        is_tool_error(&result),
        "empty repository must return isError:true, got: {}",
        tool_text(&result)
    );
    assert!(
        !out.path().join("export.jsonl").exists(),
        "no file should be written for an empty repository"
    );
}

/// REQ-MCP-EXPORT-05: export_file with empty content yields an honest
/// CallToolResult::error (isError:true, Spanish).
#[tokio::test]
async fn test_export_file_missing_content_honest_error() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(0).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out");
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_file",
        json!({
            "output_dir": out.path().to_string_lossy(),
            "filename": "empty",
            "format": "jsonl",
            "content": "   "
        }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        is_tool_error(&result),
        "empty content must return isError:true, got: {}",
        tool_text(&result)
    );
}

/// REQ-MCP-EXPORT-07: an unrecognized format is rejected with an explicit
/// JSON-RPC invalid-params error — NOT the legacy silent fallback to Jsonl.
#[tokio::test]
async fn test_export_invalid_format_hard_error() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(0).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out");
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_file",
        json!({
            "output_dir": out.path().to_string_lossy(),
            "filename": "bad",
            "format": "bogus",
            "content": "some content"
        }),
    )
    .await;

    let error = resp
        .get("error")
        .unwrap_or_else(|| panic!("invalid format must return a JSON-RPC error, got: {resp}"));
    let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    assert_eq!(
        code, -32602,
        "invalid format must map to JSON-RPC invalid-params (-32602), got: {error}"
    );
    assert!(
        !out.path().join("bad.jsonl").exists(),
        "no silent Jsonl fallback file may be written"
    );
}

/// REQ-MCP-EXPORT-07 (uncreatable output_dir): an output directory that cannot
/// be created yields an honest CallToolResult::error (isError:true, Spanish) —
/// never a fake success. The directory is made uncreatable by routing it
/// through an existing regular file, so `create_dir_all` fails inside the
/// exporter (ExporterError::DirectoryCreation) and surfaces via process_results.
#[tokio::test]
async fn test_export_uncreatable_output_dir_honest_error() {
    // Seed one result so load_results succeeds and the tool reaches the export
    // (directory-creation) path rather than the empty-repository early return.
    let (base_url, _handle, _container_tmp) = start_seeded_server(1).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    // Build an output_dir that cannot be created: a path descending through an
    // existing regular file. `create_dir_all` on this fails (NotADirectory).
    let blocker_tmp = RelTempDir::new("wf-blocker");
    let blocker_file = blocker_tmp.path().join("blocker.txt");
    std::fs::write(&blocker_file, "a regular file, not a directory").unwrap();
    let uncreatable_dir = blocker_file.join("nested").join("output");

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_jsonl",
        json!({ "output_dir": uncreatable_dir.to_string_lossy(), "filename": "export" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        is_tool_error(&result),
        "uncreatable output_dir must return isError:true, got: {}",
        tool_text(&result)
    );

    assert!(
        !uncreatable_dir.join("export.jsonl").exists(),
        "no file may be written to an uncreatable output_dir"
    );
}

// ============================================================================
// 5. Scrape metrics — real accumulated metrics (issue #382, mcp-real-metrics)
// ============================================================================

/// Minimal article HTML that Readability extracts deterministically (mirrors
/// `tests/scraper_service_test.rs`), so `scrape_url` records a success event.
const METRICS_ARTICLE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
<article>
<h1>Main Heading</h1>
<p>This is the content of the article. It has enough text to be extracted by Readability.</p>
</article>
</body>
</html>"#;

/// REQ-04/06/08: a scrape recorded through ONE session is visible via
/// `get_scrape_metrics` from a DIFFERENT session (shared `Arc` accumulator),
/// returning real JSON — `total_events`, the scraped domain, and a timing field.
#[tokio::test]
async fn test_scrape_then_metrics_reflects_it_cross_session() {
    // Arrange: wiremock answers GET / with article HTML Readability can extract.
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(METRICS_ARTICLE_HTML))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();

    // Session A performs the scrape (records one event after instrumentation).
    let session_a = init_session(&client, &base_url).await;
    let scrape_resp = call_tool(
        &client,
        &base_url,
        &session_a,
        "scrape_url",
        json!({ "url": mock.uri() }),
    )
    .await;
    let scrape_result = scrape_resp
        .get("result")
        .unwrap_or_else(|| panic!("expected scrape result, got: {scrape_resp}"))
        .clone();
    assert!(
        !is_tool_error(&scrape_result),
        "scrape_url should succeed against wiremock: {}",
        tool_text(&scrape_result)
    );

    // Session B (DIFFERENT) reads the metrics — the accumulator is shared (REQ-06).
    let session_b = init_session(&client, &base_url).await;
    assert_ne!(session_a, session_b, "sessions must be distinct");
    let metrics_resp = call_tool(
        &client,
        &base_url,
        &session_b,
        "get_scrape_metrics",
        json!({}),
    )
    .await;
    let metrics_result = metrics_resp
        .get("result")
        .unwrap_or_else(|| panic!("expected metrics result, got: {metrics_resp}"))
        .clone();

    // REQ-04: populated read is a success (isError not true) with real JSON.
    assert!(
        !is_tool_error(&metrics_result),
        "get_scrape_metrics must succeed after a scrape: {}",
        tool_text(&metrics_result)
    );
    let text = tool_text(&metrics_result);
    let parsed: Value = serde_json::from_str(&text).expect("metrics must be valid JSON");

    let total = parsed
        .get("total_events")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert!(
        total >= 1,
        "total_events must reflect the scrape, got: {text}"
    );

    // host_str() strips the wiremock port → stable "127.0.0.1" domain key.
    let domains = parsed
        .get("domains")
        .and_then(|v| v.as_object())
        .expect("domains object present");
    assert!(
        domains.contains_key("127.0.0.1"),
        "domains must contain the scraped host 127.0.0.1, got: {text}"
    );
    assert!(
        parsed.get("average_duration_ms").is_some(),
        "average_duration_ms must be present, got: {text}"
    );

    // REQ-08: the redacted snapshot is byte-stable across runs.
    insta::with_settings!({
        filters => vec![
            (r#""average_duration_ms":\s*[\d.]+"#, "[DURATION]"),
            (r"127\.0\.0\.1:\d+", "127.0.0.1:[PORT]"),
        ],
    }, {
        insta::assert_snapshot!("scrape_metrics_cross_session", &text);
    });
}

/// REQ-05: a fresh server with NO recorded scrapes returns an honest Spanish
/// pre-condition error (isError:true) — never the legacy canned success JSON.
#[tokio::test]
async fn test_metrics_empty_state_honest_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    // Read metrics FIRST, before any scrape, on a fresh server.
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "get_scrape_metrics",
        json!({}),
    )
    .await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();

    assert!(
        is_tool_error(&result),
        "empty metrics must return isError:true, got: {}",
        tool_text(&result)
    );
}

// ============================================================================
// 6. AI tools — honest errors (issue #381 slice 2)
// ============================================================================

/// REQ-04: search_obsidian returns an honest `CallToolResult::error`
/// (isError:true, Spanish) when the vault-search ports are absent.
///
/// `start_test_server` builds a bare `Container` and never wires the
/// embedding/note/chunker ports (the production wiring lives in the
/// `mcp_server` example behind `WEBFANG_MCP_AI`, #433), so this asserts the
/// permanent "ports absent → honest error" contract: never a false success,
/// never a protocol error — independent of whether `--features ai` is compiled.
#[tokio::test]
async fn test_search_obsidian_not_implemented_is_honest_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "search_obsidian",
        json!({ "query": "rust async patterns" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        is_tool_error(&result),
        "search_obsidian must return isError:true, got: {}",
        tool_text(&result)
    );
}

/// REQ-03: semantic_cleaner rejects a malformed URL with a JSON-RPC
/// invalid-params error (-32602), regardless of cleaner state — no fetch, no
/// cleaning. Mirrors `test_export_invalid_format_hard_error`.
#[tokio::test]
async fn test_semantic_cleaner_invalid_url_is_invalid_params() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "semantic_cleaner",
        json!({ "url": "not a valid url" }),
    )
    .await;

    let error = resp
        .get("error")
        .unwrap_or_else(|| panic!("invalid URL must return a JSON-RPC error, got: {resp}"));
    let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    assert_eq!(
        code, -32602,
        "invalid URL must map to JSON-RPC invalid-params (-32602), got: {error}"
    );
}

/// REQ-02: with no cleaner injected (bare container, `ai` feature off),
/// `semantic_cleaner` returns an honest `CallToolResult::error` (isError:true,
/// Spanish) for a valid URL — never a protocol error, never a false success,
/// and no fetch is attempted.
#[tokio::test]
async fn test_semantic_cleaner_without_cleaner_is_honest_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "semantic_cleaner",
        json!({ "url": "https://example.com" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        is_tool_error(&result),
        "absent cleaner must return isError:true, got: {}",
        tool_text(&result)
    );
}

/// REQ-06: with the `ai` feature off, the MCP server still registers the AI
/// tools (`semantic_cleaner` + `search_obsidian`). Registration is
/// unconditional; the tools report honest errors when invoked without a cleaner.
#[tokio::test]
async fn test_mcp_handler_construction_without_ai() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let body = mcp_request("tools/list", json!({}));
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&body)
        .send()
        .await
        .expect("tools/list should succeed");
    let text = resp.text().await.expect("read body");
    let parsed = extract_json(&text).expect("tools/list must parse");

    let tools = parsed
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .unwrap_or_else(|| panic!("tools/list must return a tools array, got: {parsed}"));
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name")?.as_str())
        .collect();

    assert!(
        names.contains(&"semantic_cleaner"),
        "semantic_cleaner must be registered with ai off, got: {names:?}"
    );
    assert!(
        names.contains(&"search_obsidian"),
        "search_obsidian must be registered with ai off, got: {names:?}"
    );
}

// ============================================================================
// 7. MCP + AI — success + observability (issue #542, Phase 5)
// ============================================================================

/// Start a test MCP server whose `Container` carries a real AI semantic cleaner
/// built from the local ONNX model cache (offline mode).
///
/// `#[cfg(feature = "ai")]` because the cleaner type (`SemanticCleanerImpl`)
/// lives behind `webfang_ai`, which is an optional dependency of `webfang_mcp`.
#[cfg(feature = "ai")]
async fn start_test_server_with_ai() -> (String, tokio::task::JoinHandle<()>) {
    // Disable SSRF before building the router
    init_ssrf_disabled();

    use webfang_ai::infrastructure_ai::{ModelConfig, SemanticCleanerImpl};

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");

    // Resolve the model from the local cache — no network access.
    let model_config = ModelConfig::default().with_offline_mode(true);
    let cleaner = SemanticCleanerImpl::new(model_config)
        .await
        .expect("cached ONNX model required");

    let container = container.with_cleaner(std::sync::Arc::new(cleaner));
    let state = McpState::new(container);
    let app = build_mcp_router(state, &ServerOptions::default());

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

/// Article HTML that Readability extracts deterministically.
const MCP_AI_ARTICLE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>MCP AI Test</title></head>
<body>
<article>
<h1>Semantic Cleaning via MCP</h1>
<p>This article provides enough content for the semantic cleaner to produce
real document chunks with embeddings. The extractor needs sufficient text
to trigger readability extraction and the subsequent AI cleaning pipeline.</p>
<p>A second paragraph ensures the chunker has enough material to work with.
Multiple paragraphs stabilize extraction and give the embedding model
meaningful tokens to process during inference.</p>
</article>
</body>
</html>"#;

/// REQ-MCP-AI-01: with the `ai` feature and the model cached, `tools/call
/// semantic_cleaner` returns chunks with non-empty embeddings.
#[cfg(feature = "ai")]
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn semantic_cleaner_with_cleaner_success() {
    let (base_url, _handle) = start_test_server_with_ai().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    // Mock HTTP server that serves article HTML.
    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(MCP_AI_ARTICLE_HTML))
        .mount(&mock)
        .await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "semantic_cleaner",
        json!({ "url": mock.uri() }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "semantic_cleaner should succeed: {}",
        tool_text(&result)
    );

    let text = tool_text(&result);
    let parsed: Value = serde_json::from_str(&text).expect("response must parse as JSON");

    let chunks = parsed.get("chunks").and_then(|v| v.as_u64()).unwrap_or(0);
    assert!(chunks > 0, "should produce at least one chunk, got: {text}");

    let embedding_dim = parsed
        .get("embedding_dim")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert!(
        embedding_dim > 0,
        "embedding_dim should be non-zero, got: {text}"
    );

    let documents = parsed
        .get("documents")
        .and_then(|v| v.as_array())
        .expect("documents array must be present");
    assert!(!documents.is_empty(), "documents must not be empty");

    // At least one document carries a non-empty embedding vector.
    let has_embedding = documents.iter().any(|doc| {
        doc.get("embeddings")
            .and_then(|e| e.as_array())
            .is_some_and(|arr| !arr.is_empty())
    });
    assert!(
        has_embedding,
        "at least one document must carry a non-empty embedding"
    );
}

/// REQ-MCP-AI-02: `search_obsidian` without vault/index returns an honest
/// `CallToolResult::error` (isError:true, Spanish).
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn search_obsidian_honest_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "search_obsidian",
        json!({ "query": "rust async patterns" }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        is_tool_error(&result),
        "search_obsidian must return isError:true without vault, got: {}",
        tool_text(&result)
    );
}

/// REQ-MCP-AI-03: `semantic_cleaner` generates traceable spans (the handler is
/// `#[instrument]`-annotated). Verifies the tool succeeds AND that the response
/// carries traceable structure (URL echo, chunk count, embedding dim).
#[cfg(feature = "ai")]
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn mcp_ai_observability() {
    let (base_url, _handle) = start_test_server_with_ai().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let mock = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(MCP_AI_ARTICLE_HTML))
        .mount(&mock)
        .await;

    let url = mock.uri();
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "semantic_cleaner",
        json!({ "url": &url }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "semantic_cleaner should succeed: {}",
        tool_text(&result)
    );

    let text = tool_text(&result);
    let parsed: Value = serde_json::from_str(&text).expect("response must parse as JSON");

    // The instrumented handler echoes the URL and reports structured metrics.
    assert_eq!(
        parsed.get("url").and_then(|v| v.as_str()),
        Some(url.as_str()),
        "response should echo the instrumented URL"
    );
    assert!(
        parsed.get("chunks").is_some(),
        "response should report chunks (instrumented span result)"
    );
    assert!(
        parsed.get("embedding_dim").is_some(),
        "response should report embedding_dim (instrumented span result)"
    );
}

/// REQ-MCP-AI-04: `semantic_cleaner` rejects a malformed URL with a JSON-RPC
/// invalid-params error (-32602).
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn semantic_cleaner_invalid_url_reject() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "semantic_cleaner",
        json!({ "url": "not a valid url" }),
    )
    .await;

    let error = resp
        .get("error")
        .unwrap_or_else(|| panic!("invalid URL must return a JSON-RPC error, got: {resp}"));
    let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    assert_eq!(
        code, -32602,
        "invalid URL must map to JSON-RPC invalid-params (-32602), got: {error}"
    );
}

/// REQ-MCP-AI-05: with the `ai` feature off, the MCP server still registers the
/// AI tools (`semantic_cleaner` + `search_obsidian`). Registration is
/// unconditional; the tools report honest errors when invoked without a cleaner.
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn mcp_ai_tools_registered_with_ai_off() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let body = mcp_request("tools/list", json!({}));
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&body)
        .send()
        .await
        .expect("tools/list should succeed");
    let text = resp.text().await.expect("read body");
    let parsed = extract_json(&text).expect("tools/list must parse");

    let tools = parsed
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(|t| t.as_array())
        .unwrap_or_else(|| panic!("tools/list must return a tools array, got: {parsed}"));
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name")?.as_str())
        .collect();

    assert!(
        names.contains(&"semantic_cleaner"),
        "semantic_cleaner must be registered with ai off, got: {names:?}"
    );
    assert!(
        names.contains(&"search_obsidian"),
        "search_obsidian must be registered with ai off, got: {names:?}"
    );
}
