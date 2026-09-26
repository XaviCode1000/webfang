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
use wreq::Client;

mod common;
use common::{call_tool, init_session, is_tool_error, start_test_server_ssrf_enabled, tool_text};

/// Assert a WAF block verdict: the tool succeeds (`isError:false`) and its
/// content begins with the stable ENGLISH prefix "WAF blocked:" (see
/// crates/webfang_mcp/src/mcp_server/handlers/security.rs). No JSON-RPC
/// `error.code` is produced on this path — the handler returns
/// `Ok(CallToolResult::success(...))` — so we assert on the stable prefix
/// rather than any localized message text. `label` disambiguates the call site.
fn assert_waf_blocked(result: &Value, label: &str) {
    assert!(
        !is_tool_error(result),
        "verify_waf_integrity should succeed: {}",
        tool_text(result)
    );
    assert!(
        // #1600: the verdict text derives from caller HTML (RemoteDerived) and
        // carries the provenance envelope plus its one-space indentation.
        tool_text(result).trim().starts_with("WAF blocked:"),
        "{label}, got: {}",
        tool_text(result)
    );
}

// ============================================================================
// detect_waf — degraded mode (no HTTP context)
// ============================================================================

/// A Challenge-tier (T1) marker blocks even in degraded mode.
#[tokio::test]
async fn test_detect_waf_challenge_marker_is_detected() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
    assert_eq!(
        tool_text(&result).trim(),
        "WAF detected: Cloudflare Turnstile"
    );
}

/// A bare vendor fingerprint (T2) is evidence only and NEVER blocks in
/// degraded mode — the issue #346 false-positive fix.
#[tokio::test]
async fn test_detect_waf_bare_fingerprint_never_blocks_degraded() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
    assert_eq!(tool_text(&result).trim(), "no WAF detected");
}

/// A clean body reports no WAF.
#[tokio::test]
async fn test_detect_waf_clean_body_no_waf() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
    assert_eq!(tool_text(&result).trim(), "no WAF detected");
}

// ============================================================================
// verify_waf_integrity — degraded mode + additive context
// ============================================================================

/// Degraded mode (no status/content_type): a control header (T2 fingerprint)
/// alone never blocks on mere presence — evidence is collected but the check
/// passes. This pins the issue #346 verdict change end-to-end.
#[tokio::test]
async fn test_verify_waf_integrity_header_alone_passes_degraded() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
        tool_text(&result).trim(),
        "WAF integrity check passed",
        "T2 header alone must not block in degraded mode (#346)"
    );
}

/// A Challenge-tier (T1) marker blocks even without HTTP context.
#[tokio::test]
async fn test_verify_waf_integrity_t1_challenge_blocks_degraded() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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

    assert_waf_blocked(&result, "T1 challenge must block");
}

/// Additive context: a bare vendor fingerprint (T2) blocks when a correlated
/// WAF status (403) is supplied.
#[tokio::test]
async fn test_verify_waf_integrity_t2_with_waf_status_blocks() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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

    assert_waf_blocked(&result, "T2 fingerprint + WAF status 403 must block");
}

/// Additive context: the SAME T2 body at an OK status (200) passes.
#[tokio::test]
async fn test_verify_waf_integrity_t2_with_ok_status_passes() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
        tool_text(&result).trim(),
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
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
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
