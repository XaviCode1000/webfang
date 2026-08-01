//! MCP `download_assets` behavioral tests
//!
//! Proves the tool is wired into the real `AssetDownloaderPort` infrastructure
//! (#452): a wiremock server serves the assets, a real `Downloader` writes
//! SHA-256 hashed files into a TempDir, and the tool returns them as JSON.
//!
//! Run with: cargo nextest run --test download_assets_test --features mcp

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wreq::Client;

use webfang_core::adapters::downloader::{DownloadConfig, Downloader};
use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::state::McpState;

/// Start a test MCP server on a random port. Pass `Some(downloader)` to inject
/// a shared `AssetDownloaderPort` (the production wiring in `mcp_server.rs`);
/// pass `None` to exercise the per-call fallback downloader built from config.
async fn start_server(
    downloader: Option<Arc<Downloader>>,
) -> (String, tokio::task::JoinHandle<()>) {
    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = match downloader {
        Some(d) => McpState::new(container).with_downloader(d),
        None => McpState::new(container),
    };
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

/// Initialize an MCP session (initialize + notifications/initialized) and
/// return the session ID.
async fn init_session(client: &Client, base_url: &str) -> String {
    let init_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "download-assets-test", "version": "1.0.0" }
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

/// Unwrap the CallToolResult object from the JSON-RPC envelope returned by
/// `call_tool`, so helpers like `tool_text` see the `content` field.
fn tool_result(resp: Value) -> Value {
    resp.get("result")
        .cloned()
        .expect("JSON-RPC response must carry a result")
}

/// Parse the tool's JSON text payload into an array of downloaded assets.
fn asset_array(result: &Value) -> Vec<Value> {
    let parsed: Value = serde_json::from_str(&tool_text(result)).expect("response is JSON");
    parsed.as_array().expect("response is an array").to_vec()
}

#[tokio::test]
async fn download_assets_downloads_images_via_shared_downloader() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/logo.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(b"PNG-BYTES"),
        )
        .mount(&mock)
        .await;

    let out = tempfile::TempDir::new().expect("create output temp dir");
    let dl_config = DownloadConfig {
        output_dir: out.path().to_path_buf(),
        ..Default::default()
    };
    let downloader = Arc::new(Downloader::new(dl_config).expect("build downloader"));
    let (base_url, _handle) = start_server(Some(downloader)).await;

    let client = Client::new();
    let session = init_session(&client, &base_url).await;

    let html = format!(
        "<html><body><img src=\"{}/logo.png\"></body></html>",
        mock.uri()
    );
    let result = tool_result(
        call_tool(
            &client,
            &base_url,
            &session,
            "download_assets",
            json!({
                "html": html,
                "base_url": mock.uri(),
                "images": true,
                "documents": false,
            }),
        )
        .await,
    );

    assert!(
        !is_tool_error(&result),
        "expected success, got: {}",
        tool_text(&result)
    );
    let assets = asset_array(&result);
    assert_eq!(assets.len(), 1, "one image expected");
    assert_eq!(assets[0]["asset_type"], "image");

    let local_path = assets[0]["local_path"].as_str().expect("local_path");
    let file_name = std::path::Path::new(local_path)
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .to_string();
    // SHA-256 hashed filename: <12 hex chars>.png
    assert_eq!(file_name.len(), 16, "unexpected filename {file_name}");
    assert!(
        file_name[..12].chars().all(|c| c.is_ascii_hexdigit()),
        "hash prefix must be hex, got {file_name}"
    );
    assert!(file_name.ends_with(".png"));
    assert!(
        std::path::Path::new(local_path).exists(),
        "asset file must exist on disk"
    );

    // Deterministic snapshot of the full tool response.
    insta::with_settings!({
        filters => vec![
            (out.path().to_string_lossy().as_ref(), "[OUT_DIR]"),
            (r"127\.0\.0\.1:\d+", "127.0.0.1:[PORT]"),
        ],
    }, {
        insta::assert_snapshot!("download_assets_images_response", tool_text(&result));
    });
}

#[tokio::test]
async fn download_assets_downloads_documents_when_enabled() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/report.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/pdf")
                .set_body_bytes(b"%PDF-1.4 fake pdf"),
        )
        .mount(&mock)
        .await;

    let out = tempfile::TempDir::new().expect("create output temp dir");
    let dl_config = DownloadConfig {
        output_dir: out.path().to_path_buf(),
        ..Default::default()
    };
    let downloader = Arc::new(Downloader::new(dl_config).expect("build downloader"));
    let (base_url, _handle) = start_server(Some(downloader)).await;

    let client = Client::new();
    let session = init_session(&client, &base_url).await;

    let html = format!(
        "<html><body><a href=\"{}/report.pdf\">Report</a></body></html>",
        mock.uri()
    );
    let result = tool_result(
        call_tool(
            &client,
            &base_url,
            &session,
            "download_assets",
            json!({
                "html": html,
                "base_url": mock.uri(),
                "images": false,
                "documents": true,
            }),
        )
        .await,
    );

    assert!(
        !is_tool_error(&result),
        "expected success, got: {}",
        tool_text(&result)
    );
    let assets = asset_array(&result);
    assert_eq!(assets.len(), 1, "one document expected");
    assert_eq!(assets[0]["asset_type"], "document");

    let local_path = assets[0]["local_path"].as_str().expect("local_path");
    let file_name = std::path::Path::new(local_path)
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .to_string();
    assert!(
        file_name.ends_with(".pdf"),
        "unexpected filename {file_name}"
    );
    assert!(
        std::path::Path::new(local_path).exists(),
        "asset file must exist on disk"
    );
}

#[tokio::test]
async fn download_assets_both_disabled_returns_empty() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/logo.png"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"PNG"))
        .mount(&mock)
        .await;

    let out = tempfile::TempDir::new().expect("create output temp dir");
    let dl_config = DownloadConfig {
        output_dir: out.path().to_path_buf(),
        ..Default::default()
    };
    let downloader = Arc::new(Downloader::new(dl_config).expect("build downloader"));
    let (base_url, _handle) = start_server(Some(downloader)).await;

    let client = Client::new();
    let session = init_session(&client, &base_url).await;

    let html = format!(
        "<html><body><img src=\"{}/logo.png\"></body></html>",
        mock.uri()
    );
    let result = tool_result(
        call_tool(
            &client,
            &base_url,
            &session,
            "download_assets",
            json!({
                "html": html,
                "base_url": mock.uri(),
                "images": false,
                "documents": false,
            }),
        )
        .await,
    );

    assert!(
        !is_tool_error(&result),
        "expected success, got: {}",
        tool_text(&result)
    );
    assert!(
        asset_array(&result).is_empty(),
        "no downloads expected when both toggles are false"
    );
}

#[tokio::test]
async fn download_assets_output_dir_param_writes_to_requested_dir() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/logo.png"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "image/png")
                .set_body_bytes(b"PNG-BYTES"),
        )
        .mount(&mock)
        .await;

    // No shared downloader injected: the handler builds a per-call Downloader
    // from config, honoring the `output_dir` parameter.
    let (base_url, _handle) = start_server(None).await;

    let client = Client::new();
    let session = init_session(&client, &base_url).await;

    let out = tempfile::TempDir::new().expect("create output temp dir");
    let html = format!(
        "<html><body><img src=\"{}/logo.png\"></body></html>",
        mock.uri()
    );
    let result = tool_result(
        call_tool(
            &client,
            &base_url,
            &session,
            "download_assets",
            json!({
                "html": html,
                "base_url": mock.uri(),
                "images": true,
                "documents": false,
                "output_dir": out.path().to_string_lossy().to_string(),
            }),
        )
        .await,
    );

    assert!(
        !is_tool_error(&result),
        "expected success, got: {}",
        tool_text(&result)
    );
    let assets = asset_array(&result);
    assert_eq!(assets.len(), 1, "one image expected");

    let local_path = assets[0]["local_path"].as_str().expect("local_path");
    assert!(
        local_path.starts_with(out.path().to_string_lossy().as_ref()),
        "expected {local_path} to live under {}",
        out.path().to_string_lossy()
    );
    assert!(
        std::path::Path::new(local_path).exists(),
        "asset file must exist on disk"
    );
}
