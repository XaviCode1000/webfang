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

/// Shared arrange-act skeleton: SSRF-enabled harness + session + one tool
/// call, returning the cloned `result` member. Every case below asserts on
/// the returned value with its own shape-specific assertion, so the five-line
/// setup exists exactly once instead of once per case.
async fn call_security_tool(tool: &str, args: Value) -> Value {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;
    let resp = call_tool(&client, &base_url, &session_id, tool, args).await;
    resp.get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone()
}

/// One stable-verdict-text row: the tool, its arguments, and the exact trimmed
/// text the tool must return.
struct VerdictTextCase {
    name: &'static str,
    tool: &'static str,
    args: Value,
    expected: &'static str,
}

/// Stable verdict texts: `detect_waf` degraded-mode verdicts plus the
/// `verify_waf_integrity` passing verdicts. Every row must succeed
/// (`isError:false`) with the exact contract text — assertions are on the
/// stable prefix/exact string, never on localized message text.
fn verdict_text_cases() -> Vec<VerdictTextCase> {
    vec![
        // A Challenge-tier (T1) marker blocks even in degraded mode.
        VerdictTextCase {
            name: "detect_waf T1 challenge marker",
            tool: "detect_waf",
            args: json!({ "html": r#"<div id="cf-turnstile" data-sitekey="abc"></div>"# }),
            expected: "WAF detected: Cloudflare Turnstile",
        },
        // A bare vendor fingerprint (T2) is evidence only and NEVER blocks in
        // degraded mode — the issue #346 false-positive fix.
        VerdictTextCase {
            name: "detect_waf bare T2 fingerprint",
            tool: "detect_waf",
            args: json!({ "html": "<html><body>powered by cloudflare</body></html>" }),
            expected: "no WAF detected",
        },
        // A clean body reports no WAF.
        VerdictTextCase {
            name: "detect_waf clean body",
            tool: "detect_waf",
            args: json!({ "html": "<html><body>normal content</body></html>" }),
            expected: "no WAF detected",
        },
        // Degraded mode (no status/content_type): a control header (T2
        // fingerprint) alone never blocks on mere presence — evidence is
        // collected but the check passes. This pins the issue #346 verdict
        // change end-to-end.
        VerdictTextCase {
            name: "verify_waf_integrity T2 header alone passes degraded",
            tool: "verify_waf_integrity",
            args: json!({
                "html": "<html>clean</html>",
                "headers": { "x-datadome-response": "1" }
            }),
            expected: "WAF integrity check passed",
        },
        // Additive context: the SAME T2 body at an OK status (200) passes.
        VerdictTextCase {
            name: "verify_waf_integrity T2 body at OK status passes",
            tool: "verify_waf_integrity",
            args: json!({
                "html": "<html>blocked by akamai</html>",
                "status": 200,
                "content_type": "text/html"
            }),
            expected: "WAF integrity check passed",
        },
    ]
}

/// Tool verdict texts match the stable contract: success with the exact
/// expected text.
#[tokio::test]
async fn tool_verdict_texts_match_the_stable_contract() {
    for case in verdict_text_cases() {
        let result = call_security_tool(case.tool, case.args).await;
        assert!(
            !is_tool_error(&result),
            "{}: tool should succeed: {}",
            case.name,
            tool_text(&result)
        );
        assert_eq!(
            tool_text(&result).trim(),
            case.expected,
            "{}: unexpected verdict text",
            case.name,
        );
    }
}

/// One block-verdict row: the arguments plus the label disambiguating the call
/// site in `assert_waf_blocked`.
struct BlockCase {
    args: Value,
    label: &'static str,
}

/// Block verdicts: `verify_waf_integrity` answers success (`isError:false`)
/// with the stable ENGLISH "WAF blocked:" prefix.
fn block_cases() -> Vec<BlockCase> {
    vec![
        // A Challenge-tier (T1) marker blocks even without HTTP context.
        BlockCase {
            args: json!({ "html": "Just a moment..." }),
            label: "T1 challenge must block",
        },
        // Additive context: a bare vendor fingerprint (T2) blocks when a
        // correlated WAF status (403) is supplied.
        BlockCase {
            args: json!({
                "html": "<html>blocked by akamai</html>",
                "status": 403,
                "content_type": "text/html"
            }),
            label: "T2 fingerprint + WAF status 403 must block",
        },
    ]
}

/// Blocking verdicts succeed with the stable "WAF blocked:" prefix.
#[tokio::test]
async fn waf_block_verdicts_carry_the_stable_prefix() {
    for case in block_cases() {
        let result = call_security_tool("verify_waf_integrity", case.args).await;
        assert_waf_blocked(&result, case.label);
    }
}

// ============================================================================
// list_waf_providers
// ============================================================================

/// The provider list is real and non-empty, and includes known providers.
#[tokio::test]
async fn test_list_waf_providers_is_non_empty() {
    let result = call_security_tool("list_waf_providers", json!({})).await;

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
