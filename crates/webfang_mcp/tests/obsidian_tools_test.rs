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
use wreq::Client;

mod common;
use common::{call_tool, init_session, is_tool_error, start_test_server_ssrf_enabled, tool_text};

/// Control-character rejection is signaled by `isError:true` on the tool
/// result, NOT by the user-facing error message (may change). The handler
/// returns `Ok(CallToolResult::error(...))` — a tool-level error with no
/// `code` (see crates/webfang_mcp/src/mcp_server/handlers/obsidian.rs).
fn assert_control_chars_rejected(result: &Value) {
    assert!(
        is_tool_error(result),
        "control chars must yield isError:true, got: {}",
        tool_text(result)
    );
}

/// A relative temporary directory that deletes itself on drop.
///
/// `tempfile::TempDir` always returns an absolute path (it joins with
/// `env::current_dir`), which the MCP `require_safe_path` validator rejects.
/// These tests need a *relative* vault dir, so we manage one manually.
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
// build_obsidian_uri
// ============================================================================

/// The happy path produces the exact `obsidian://open?vault=...&file=...` URI
/// with slashes preserved (Obsidian file paths are not percent-encoded).
#[tokio::test]
async fn test_build_obsidian_uri_happy_path() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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

    assert_control_chars_rejected(&result);
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
    let vault = RelTempDir::new("wf-vault");
    std::fs::create_dir_all(vault.path().join(".obsidian")).unwrap();

    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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

    assert_control_chars_rejected(&result);
    // Rejection is signaled by `isError:true`, NOT the Spanish error message
    // (user-facing text, may change). The handler returns
    // `Ok(CallToolResult::error(...))` — a tool-level error with no `code`.
    let text = tool_text(&result);
    assert!(
        !text.contains("Opened in Obsidian"),
        "no real app launch may be reported, got: {text}"
    );
}
