//! Export tools behavioral coverage — error paths (issue #450).
//!
//! End-to-end tests for the export tools that were only partially covered:
//! - `export_vector`: empty-session honest error (isError:true, Spanish)
//! - `process_export_pipeline`: empty-session honest error + invalid-format
//!   JSON-RPC invalid-params (-32602)
//!
//! Happy paths and `export_jsonl`/`export_file` error paths are covered in
//! mcp_behavioral_test.rs.
//!
//! Run with: cargo nextest run -p webfang_mcp --features mcp --test export_coverage_test

#![cfg(feature = "mcp")]

use serde_json::json;
use wreq::Client;

mod common;
use common::{call_tool, init_session, is_tool_error, start_seeded_server, tool_text};

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

/// REQ-MCP-EXPORT-05: `export_vector` on an empty session returns an honest
/// `CallToolResult::error` (isError:true, Spanish) and writes no file.
#[tokio::test]
async fn test_export_vector_empty_session_honest_error() {
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
        "empty session must return isError:true, got: {}",
        tool_text(&result)
    );

    assert!(
        !out.path().join("vectors.json").exists(),
        "no file should be written for an empty session"
    );
}

// ============================================================================
// process_export_pipeline
// ============================================================================

/// REQ-MCP-EXPORT-05: `process_export_pipeline` on an empty session returns
/// an honest `CallToolResult::error` (isError:true, Spanish) — never a queued
/// or fake success.
#[tokio::test]
async fn test_process_export_pipeline_empty_session_honest_error() {
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
        "empty session must return isError:true, got: {}",
        tool_text(&result)
    );

    assert!(
        !container_tmp.path().join("export.jsonl").exists(),
        "no file should be written for an empty session"
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
