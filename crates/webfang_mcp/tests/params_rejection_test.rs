//! End-to-end MCP parameter-validation rejection tests (issue #512, slice 2).
//!
//! Every test starts the real MCP server and invokes a tool over HTTP, then
//! asserts on the JSON-RPC error *code* (never on message strings). Most
//! handlers wire `params.validate()?` as the first statement, so invalid
//! parameters are rejected at the protocol boundary with code `-32602`
//! (invalid params) before any network access or semaphore acquisition.
//! Exceptions: `validate_url` (returns tool-level `{"valid": false}`) and
//! `detect_obsidian_vault` (accepts absolute paths) — see bug #590.
//!
//! Layout (T7 slice-12, issue #1887): the rejection cases share one
//! arrange-act skeleton ([`invoke`]) and are grouped into three case tables —
//! unsupported URL schemes, `-32602` protocol rejections, and acceptance
//! controls — each iterated by a single loop test. The unknown-field
//! deserialization case keeps its own test: it asserts the tool-error channel
//! (`isError: true`), not `-32602`. Case count: 24 before (one test per case),
//! 24 after (4 + 13 + 6 table rows + 1 specialized test).
//!
//! Run with: cargo nextest run --test params_rejection_test --features mcp

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use wreq::Client;

// Session/tool-call JSON-RPC helpers live in the shared harness (`tests/common`),
// not here — issue #1371. Imported by name rather than `use common::*` so this
// file's own helpers cannot collide with `common`'s.
mod common;
use common::{
    call_tool, error_code, init_session, is_tool_error, start_test_server_ssrf_enabled, tool_text,
    JSONRPC_INVALID_PARAMS,
};

/// `validation::MAX_BLOB_LEN` (1_048_576) + 1, to exceed the max blob length.
const MAX_BLOB_LEN_PLUS_1: usize = 1_048_577;

// ============================================================================
// Harness — only the starter stays local (see its note); the JSON-RPC
// helpers come from `tests/common/mod.rs`.
// ============================================================================

/// Shared arrange-act skeleton: start the SSRF-enabled harness, open a
/// session, and invoke one tool. Every case table below iterates through this
/// single helper, so the four-line setup exists exactly once instead of once
/// per rejection case.
async fn invoke(tool: &str, args: Value) -> Value {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;
    call_tool(&client, &base_url, &session_id, tool, args).await
}

/// #1116: an invalid URL is now rejected at the `McpUrl` deserialization
/// boundary. rmcp 1.8.0 surfaces tool-ARGUMENT deserialization failures as a
/// `CallToolResult::error` (`isError:true`) rather than a JSON-RPC protocol
/// error (see `into_tool_argument_error` in rmcp's router). This helper
/// accepts EITHER rejection shape and asserts the reason names the scheme —
/// the invariant is "rejected before any fetch", not the exact envelope.
/// `case` names the table row so a failure identifies which input failed.
fn assert_url_argument_rejected(case: &str, resp: &Value, reason_substring: &str) {
    let rejected_as_protocol_error = error_code(resp) == Some(JSONRPC_INVALID_PARAMS);
    let result = resp.get("result");
    let rejected_as_tool_error = result
        .and_then(|r| r.get("isError"))
        .and_then(|v| v.as_bool())
        == Some(true);
    assert!(
        rejected_as_protocol_error || rejected_as_tool_error,
        "{case}: invalid URL must be rejected (protocol -32602 or tool isError), got: {resp}"
    );
    let text = result.map(tool_text).unwrap_or_default();
    let protocol_msg = resp
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let haystack = format!("{text}{protocol_msg}").to_lowercase();
    assert!(
        haystack.contains(&reason_substring.to_lowercase()),
        "{case}: rejection must mention '{reason_substring}', got: {resp}"
    );
}

// ============================================================================
// Rejection tests — invalid parameters must map to JSON-RPC -32602 and never
// reach network/IO.
// ============================================================================

/// One unsupported-scheme row: the tool, the offending arguments, and the row
/// name used to identify failures.
struct SchemeRejectionCase {
    name: &'static str,
    tool: &'static str,
    args: Value,
}

/// Unsupported-scheme rows: `scrape_url`, `extract_domain` and `crawl_site`
/// reject non-http(s) URLs before any fetch, on either rejection channel.
fn unsupported_scheme_cases() -> Vec<SchemeRejectionCase> {
    vec![
        SchemeRejectionCase {
            name: "scrape_url rejects file://",
            tool: "scrape_url",
            args: json!({ "url": "file:///etc/passwd" }),
        },
        SchemeRejectionCase {
            name: "scrape_url rejects ftp://",
            tool: "scrape_url",
            args: json!({ "url": "ftp://example.com/file" }),
        },
        SchemeRejectionCase {
            name: "extract_domain rejects file://",
            tool: "extract_domain",
            args: json!({ "url": "file:///etc/passwd" }),
        },
        SchemeRejectionCase {
            name: "crawl_site rejects ftp://",
            tool: "crawl_site",
            args: json!({ "url": "ftp://example.com" }),
        },
    ]
}

/// Unsupported URL schemes are rejected before any fetch, on either rejection
/// channel, with the reason naming the scheme.
#[tokio::test]
async fn unsupported_url_schemes_are_rejected_before_any_fetch() {
    for case in unsupported_scheme_cases() {
        let resp = invoke(case.tool, case.args).await;
        assert_url_argument_rejected(case.name, &resp, "no soportado");
    }
}

/// One `-32602` protocol-rejection row: the tool, the offending arguments, and
/// the assertion message naming the violated bound (without the trailing
/// response dump — the loop appends it).
struct ProtocolRejectionCase {
    name: &'static str,
    tool: &'static str,
    args: Value,
    message: &'static str,
}

/// Handler-level `params.validate()?` rejections: each row must map to
/// JSON-RPC `-32602` before any network access or semaphore acquisition.
/// Assembled from the per-area tables below (split so each stays under the
/// `too_many_lines` ratchet).
fn protocol_rejection_cases() -> Vec<ProtocolRejectionCase> {
    let mut cases = crawl_protocol_rejection_cases();
    cases.extend(export_protocol_rejection_cases());
    cases.extend(tool_protocol_rejection_cases());
    cases
}

/// Crawl/download rows of the `-32602` table.
fn crawl_protocol_rejection_cases() -> Vec<ProtocolRejectionCase> {
    vec![
        ProtocolRejectionCase {
            name: "crawl_site max_depth beyond limit",
            tool: "crawl_site",
            args: json!({ "url": "https://example.com", "max_depth": 11 }),
            message: "max_depth > 10 must be rejected with -32602",
        },
        // Absolute `checkpoint_dir` with no export roots (#1588): the
        // checkpoint is a filesystem write target and runs through the same
        // fail-closed root gate as `output_dir`. The accepted counterpart
        // (absolute `checkpoint_dir` under a configured root) lives in
        // `scraping_coverage_test.rs::mcp_crawl_checkpoint_resume_roundtrip`,
        // whose harness declares the system temp dir as its root.
        ProtocolRejectionCase {
            name: "crawl_site absolute checkpoint_dir without roots",
            tool: "crawl_site",
            args: json!({
                "url": "https://example.com",
                "max_depth": 1,
                "max_pages": 1,
                "checkpoint_dir": "/tmp/webfang-checkpoints-1588"
            }),
            message: "absolute checkpoint_dir without export roots must be rejected with -32602",
        },
        ProtocolRejectionCase {
            name: "download_assets output_dir traversal",
            tool: "download_assets",
            args: json!({
                "html": "<img src='https://example.com/a.png'>",
                "base_url": "https://example.com",
                "images": true, "documents": false, "output_dir": "../escape"
            }),
            message: "download_assets output_dir traversal must be rejected with -32602",
        },
    ]
}

/// Export rows of the `-32602` table.
fn export_protocol_rejection_cases() -> Vec<ProtocolRejectionCase> {
    vec![
        ProtocolRejectionCase {
            name: "export_file output_dir traversal",
            tool: "export_file",
            args: json!({
                "output_dir": "../escape", "filename": "out",
                "format": "jsonl", "content": "hello"
            }),
            message: "output_dir traversal must be rejected with -32602",
        },
        // Absolute `output_dir` with no export roots (#756, completing #696):
        // issue #600 relaxed the syntactic validator so absolute paths reach
        // the handler, but the root-of-trust gate (#696) was only wired into
        // `download_assets` — this tool happily wrote to any absolute
        // directory (RIESGO-MCP-EXPORT-001). The handler now enforces the
        // fail-closed gate: with no `--export-roots` configured, an absolute
        // `output_dir` is a protocol-level `-32602`.
        ProtocolRejectionCase {
            name: "export_file absolute output_dir without roots",
            tool: "export_file",
            args: json!({
                "output_dir": "/tmp/webfang-export", "filename": "out",
                "format": "jsonl", "content": "hello"
            }),
            message:
                "absolute output_dir without configured export roots must be rejected with -32602",
        },
        // #756 runtime probe on `export_jsonl`: the root-of-trust gate runs
        // BEFORE `load_results`, so an absolute `output_dir` on a server
        // without seeds/export roots yields the gate's `-32602`, not the
        // operational "no hay resultados disponibles" error.
        ProtocolRejectionCase {
            name: "export_jsonl absolute output_dir without roots",
            tool: "export_jsonl",
            args: json!({ "output_dir": "/tmp/webfang-export", "filename": "out" }),
            message:
                "absolute output_dir without configured export roots must be rejected with -32602",
        },
        // #756: same runtime proof for `export_vector` (see `export_jsonl`).
        ProtocolRejectionCase {
            name: "export_vector absolute output_dir without roots",
            tool: "export_vector",
            args: json!({ "output_dir": "/tmp/webfang-export", "filename": "out" }),
            message:
                "absolute output_dir without configured export roots must be rejected with -32602",
        },
        ProtocolRejectionCase {
            name: "export_file filename traversal (issue #601)",
            tool: "export_file",
            args: json!({
                "output_dir": "exports", "filename": "../escape",
                "format": "jsonl", "content": "hello"
            }),
            message: "filename traversal must be rejected with -32602",
        },
        ProtocolRejectionCase {
            name: "export_file filename subdirectory (issue #601)",
            tool: "export_file",
            args: json!({
                "output_dir": "exports", "filename": "sub/out",
                "format": "jsonl", "content": "hello"
            }),
            message: "filename subdirectory must be rejected with -32602",
        },
        ProtocolRejectionCase {
            name: "export_file unknown format",
            tool: "export_file",
            args: json!({
                "output_dir": "exports", "filename": "out",
                "format": "bogus", "content": "hello"
            }),
            message: "unknown format must be rejected with -32602",
        },
    ]
}

/// Remaining tool rows of the `-32602` table.
fn tool_protocol_rejection_cases() -> Vec<ProtocolRejectionCase> {
    vec![
        ProtocolRejectionCase {
            name: "build_obsidian_uri traversal file_path",
            tool: "build_obsidian_uri",
            args: json!({ "vault_name": "MyVault", "file_path": "../escape" }),
            message: "file_path traversal must be rejected with -32602",
        },
        ProtocolRejectionCase {
            name: "clean_html oversize blob",
            tool: "clean_html",
            args: json!({ "html": "a".repeat(MAX_BLOB_LEN_PLUS_1) }),
            message: "oversize html blob must be rejected with -32602",
        },
        ProtocolRejectionCase {
            name: "detect_waf oversize html (no network access)",
            tool: "detect_waf",
            args: json!({ "html": "a".repeat(MAX_BLOB_LEN_PLUS_1) }),
            message: "oversize html blob must be rejected with -32602",
        },
    ]
}

/// Handler-level validation failures map to JSON-RPC `-32602` and never reach
/// network/IO.
#[tokio::test]
async fn invalid_params_are_rejected_with_32602() {
    for case in protocol_rejection_cases() {
        let resp = invoke(case.tool, case.args).await;
        assert_eq!(
            error_code(&resp),
            Some(JSONRPC_INVALID_PARAMS),
            "{}: {}, got: {resp}",
            case.name,
            case.message,
        );
    }
}

/// `scrape_with_options` rejects an unknown field (deny_unknown_fields) at the
/// deserialization boundary, mapped to -32602.
#[tokio::test]
async fn scrape_with_options_rejects_unknown_field() {
    let resp = invoke(
        "scrape_with_options",
        json!({ "url": "https://example.com", "typo_field": 1 }),
    )
    .await;

    // rmcp 1.8.0 surfaces deserialization failures (deny_unknown_fields) as a
    // tool-level error (isError:true), not a JSON-RPC -32602 protocol error.
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected a tool result, got: {resp}"));
    assert!(
        is_tool_error(result),
        "unknown field must be rejected (deny_unknown_fields), got: {}",
        tool_text(result)
    );
}

// ============================================================================
// Control tests — valid parameters must NOT be rejected (-32602 absent) and
// must reach the tool body (proving validation does not over-reject).
// ============================================================================

/// One acceptance row: valid parameters that must NOT be rejected. Rows with
/// `check_body` additionally assert the tool result itself is not an error.
struct AcceptCase {
    name: &'static str,
    tool: &'static str,
    args: Value,
    check_body: bool,
}

/// Acceptance rows: bug-#7/#8 fixtures plus the over-rejection guards.
fn accept_cases() -> Vec<AcceptCase> {
    vec![
        // Bug #7 fix: `validate_url` no longer calls `params.validate()?` — it
        // returns a JSON tool result for ALL inputs (`file://` is a valid URL
        // per RFC 3986, so it reports valid:true with scheme:file).
        AcceptCase {
            name: "validate_url file:// returns tool result (bug #7)",
            tool: "validate_url",
            args: json!({ "url": "file:///etc/passwd" }),
            check_body: true,
        },
        // Bug #8 fix: absolute paths are accepted (no -32602). The tool
        // returns a result (vault not found, but not a validation error).
        AcceptCase {
            name: "detect_obsidian_vault accepts absolute path (bug #8)",
            tool: "detect_obsidian_vault",
            args: json!({ "vault_path": "/tmp/some-vault" }),
            check_body: false,
        },
        AcceptCase {
            name: "validate_url accepts https",
            tool: "validate_url",
            args: json!({ "url": "https://example.com/path?q=1" }),
            check_body: true,
        },
        AcceptCase {
            name: "extract_links accepts valid html",
            tool: "extract_links",
            args: json!({
                "html": "<html><body><a href=\"/page\">link</a></body></html>",
                "base_url": "https://example.com"
            }),
            check_body: false,
        },
        // The documented format: a bare `base_domain` must NOT be rejected.
        AcceptCase {
            name: "convert_wiki_links accepts bare domain",
            tool: "convert_wiki_links",
            args: json!({ "markdown": "[link](/page)", "base_domain": "example.com" }),
            check_body: true,
        },
        // The core's `normalize_seed_host` handles full URLs, so a full-URL
        // `seed_domain` must NOT be rejected by validation.
        AcceptCase {
            name: "is_internal_link accepts full-URL seed",
            tool: "is_internal_link",
            args: json!({
                "url": "https://example.com/page",
                "seed_domain": "https://example.com"
            }),
            check_body: true,
        },
    ]
}

/// Valid parameters are NOT rejected (-32602 absent) and reach the tool body,
/// proving validation does not over-reject.
#[tokio::test]
async fn valid_params_are_not_rejected() {
    for case in accept_cases() {
        let resp = invoke(case.tool, case.args).await;
        assert_eq!(
            error_code(&resp),
            None,
            "{}: valid params must not be rejected, got: {resp}",
            case.name,
        );
        if case.check_body {
            assert!(
                !is_tool_error(resp.get("result").unwrap_or(&Value::Null)),
                "{}: result must not be an error: {}",
                case.name,
                tool_text(resp.get("result").unwrap_or(&Value::Null))
            );
        }
    }
}
