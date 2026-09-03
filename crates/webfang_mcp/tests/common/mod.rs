//! Shared MCP test harness — centralized server bootstrap + JSON-RPC helpers.
//!
//! Each integration test binary is standalone, so every test file that needs a
//! live MCP server previously duplicated ~70 lines of bootstrap (container +
//! state + axum serve + session helpers). This module centralizes that setup.
//!
//! Include it with `mod common;` (or `mod common; use common::*;`) at the top of
//! a test file, then call `common::start_test_server()` and friends. The helpers
//! are intentionally `pub` but only a subset is used by any given binary, so the
//! module carries `#![allow(dead_code)]`.
//!
//! **SSRF Note**: `start_test_server()` and related functions disable SSRF
//! protection by setting `WEBFANG_MCP_DISABLE_SSRF=1` before building the router.
//! This is required because wiremock uses 127.0.0.1 for its mock HTTP server,
//! which SSRF protection blocks by design. The single exception is
//! `start_test_server_ssrf_enabled()`, which intentionally leaves the guard ON
//! to exercise it against forbidden addresses (issue #703 integration probe).

#![allow(dead_code)]

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
        std::env::set_var("WEBFANG_MCP_DISABLE_SSRF", "1");
    });
}

/// Bind `app` to a random 127.0.0.1 port, serve it, and wait until it accepts
/// connections. Returns the base URL and the server handle.
///
/// This is the shared tail of every server starter in this module — do not
/// duplicate the bind/serve/wait sequence.
pub async fn serve_on_random_port(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
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

/// Start a test MCP server on a random port and return the base URL.
///
/// NOTE: `Container::new` creates real HTTP clients (wreq) and a real service
/// layer. This is intentional for integration tests — the container is
/// ephemeral and scoped to the test, so real infrastructure gives confidence
/// that the MCP server works end-to-end with the actual application state.
pub async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    // Disable SSRF protection for tests (uses 127.0.0.1 for wiremock)
    init_ssrf_disabled();

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);

    let app = build_mcp_router(state, &ServerOptions::default());

    serve_on_random_port(app).await
}

/// Start a test MCP server on a random port. Pass `Some(downloader)` to inject
/// a shared `AssetDownloaderPort` (the production wiring in `mcp_server.rs`);
/// pass `None` to exercise the per-call fallback downloader built from config.
pub async fn start_server(
    downloader: Option<std::sync::Arc<webfang_core::adapters::downloader::Downloader>>,
) -> (String, tokio::task::JoinHandle<()>) {
    // Disable SSRF protection for tests (uses 127.0.0.1 for wiremock)
    std::env::set_var("WEBFANG_MCP_DISABLE_SSRF", "1");

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = match downloader {
        Some(d) => McpState::new(container).with_downloader(d),
        None => McpState::new(container),
    };

    let app = build_mcp_router(state, &ServerOptions::default());

    serve_on_random_port(app).await
}

/// Start a test MCP server whose crawl-result repository is pre-seeded with
/// `n` `ScrapedContent` items.
///
/// Returns `(base_url, server_handle, container_tmp)`. The container temp dir
/// is returned so the caller keeps it alive (the append-only repository log
/// lives inside it) and so tests can locate exports that default to the
/// container's configured `output_dir` (e.g. `process_export_pipeline`).
pub async fn start_seeded_server(
    n: usize,
) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    use webfang_core::domain::config::ScraperConfig;
    use webfang_core::domain::{CrawlerConfig, ScrapedContent, ValidUrl};

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
            quality_hint: None,
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

    // Disable SSRF protection for tests (uses 127.0.0.1 for wiremock)
    std::env::set_var("WEBFANG_MCP_DISABLE_SSRF", "1");

    let app = build_mcp_router(state, &ServerOptions::default());

    let (base_url, handle) = serve_on_random_port(app).await;

    (base_url, handle, container_tmp)
}

/// Start a test MCP server with SSRF protection ENABLED.
///
/// Every other starter sets `WEBFANG_MCP_DISABLE_SSRF=1` because wiremock binds
/// 127.0.0.1; this one exists precisely to exercise the guard against those
/// forbidden addresses (issue #703 integration probe). Tests using it must
/// target IPs that fail validation BEFORE any fetch, so no mock server is
/// needed. Also actively removes the disable flag in case a shared CI/parent
/// environment exported it.
pub async fn start_test_server_ssrf_enabled() -> (String, tokio::task::JoinHandle<()>) {
    std::env::remove_var("WEBFANG_MCP_DISABLE_SSRF");

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);

    let app = build_mcp_router(state, &ServerOptions::default());

    serve_on_random_port(app).await
}

/// Build a JSON-RPC request body for MCP protocol.
pub fn mcp_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
}

/// Extract the first JSON-RPC object from an SSE (`data: ` prefixed) or direct
/// JSON response body.
pub fn extract_json(body: &str) -> Option<Value> {
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
/// run-to-run.
pub fn redact_path(text: &str, dir: &std::path::Path) -> String {
    text.replace(dir.to_string_lossy().as_ref(), "[OUT_DIR]")
}

/// Initialize an MCP session (initialize + notifications/initialized) and
/// return the session ID.
pub async fn init_session(client: &Client, base_url: &str) -> String {
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
pub async fn call_tool(
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
pub fn tool_text(result: &Value) -> String {
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
pub fn is_tool_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
