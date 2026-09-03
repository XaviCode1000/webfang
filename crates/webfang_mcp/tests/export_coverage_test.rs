//! Export tools behavioral coverage — error paths (issue #450).
//!
//! End-to-end tests for the export tools that were only partially covered:
//! - `export_vector`: empty-repository honest error (isError:true, Spanish)
//! - `process_export_pipeline`: empty-repository honest error + invalid-format
//!   JSON-RPC invalid-params (-32602)
//!
//! Happy paths and `export_jsonl`/`export_file` error paths are covered in
//! mcp_behavioral_test.rs.
//!
//! Run with: cargo nextest run -p webfang_mcp --features mcp --test export_coverage_test

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use wreq::Client;

use webfang_core::di::Container;
use webfang_core::domain::{CrawlerConfig, ScrapedContent, ValidUrl};
use webfang_core::domain::config::ScraperConfig;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::server::ServerOptions;
use webfang_mcp::mcp_server::state::McpState;

// ============================================================================
// Harness helpers — local copies (each integration test binary is standalone).
// ============================================================================

/// Start a test MCP server whose crawl-result repository is pre-seeded with
/// `n` `ScrapedContent` items. Returns `(base_url, server_handle, container_tmp)`.
async fn start_seeded_server(n: usize) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
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
            quality_hint: None,
        };
        repo.save(&content).expect("save seeded content");
    }
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
            "clientInfo": { "name": "export-coverage-test", "version": "1.0.0" }
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
// export_vector
// ============================================================================

/// REQ-MCP-EXPORT-05: `export_vector` on an empty repository returns an honest
/// `CallToolResult::error` (isError:true, Spanish) and writes no file.
#[tokio::test]
async fn test_export_vector_empty_repo_honest_error() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(0).await;
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
        is_tool_error(&result),
        "empty repository must return isError:true, got: {}",
        tool_text(&result)
    );

    assert!(
        !out.path().join("vectors.json").exists(),
        "no file should be written for an empty repository"
    );
}

// ============================================================================
// process_export_pipeline
// ============================================================================

/// REQ-MCP-EXPORT-05: `process_export_pipeline` on an empty repository returns
/// an honest `CallToolResult::error` (isError:true, Spanish) — never a queued
/// or fake success.
#[tokio::test]
async fn test_process_export_pipeline_empty_repo_honest_error() {
    let (base_url, _handle, container_tmp) = start_seeded_server(0).await;
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
        is_tool_error(&result),
        "empty repository must return isError:true, got: {}",
        tool_text(&result)
    );

    assert!(
        !container_tmp.path().join("export.jsonl").exists(),
        "no file should be written for an empty repository"
    );
}

/// REQ-MCP-EXPORT-07: an unrecognized pipeline format is rejected with an
/// explicit JSON-RPC invalid-params error (-32602) — never a silent fallback.
#[tokio::test]
async fn test_process_export_pipeline_invalid_format_hard_error() {
    let (base_url, _handle, _container_tmp) = start_seeded_server(0).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "process_export_pipeline",
        json!({ "format": "bogus" }),
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
}
