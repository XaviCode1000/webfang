//! Transport-level proof of the EC-08 reason slug and the EC-04
//! success-shaped diagnostic channel (issue #1613, slice 5).
//!
//! Two channels, one contract. Both used to be unreadable by a machine:
//!
//! * **Channel A — JSON-RPC rejection.** `error.data` was a bare JSON string
//!   naming the field and every rejection shared the code `-32602`, so an
//!   agent could tell WHICH field was wrong but never WHY, and had to parse
//!   prose. `data` is now an object, `{"field": …, "reason": <slug>}`, where
//!   the slug is one of exactly seven stable values (the taxonomy is pinned
//!   exhaustively in `validation.rs`'s unit tests, since most slugs cannot be
//!   provoked from outside).
//! * **Channel B — `validate_url`'s `valid: false` body.** A success-shaped
//!   result (`isError` is never set) whose `reason` used to be
//!   `ErrorData::to_string()` — rmcp renders that as
//!   `"{code}: {message}({data})"`, so the human-facing field carried the
//!   numeric JSON-RPC code and a raw JSON blob. It is now the message alone,
//!   with the slug in its own `reason_code` field.
//!
//! These tests start the real MCP server and assert on the WIRE bytes only:
//! the `error` object of a JSON-RPC response, or the parsed JSON body of a
//! tool result. Nothing here reaches the network — every case is rejected at
//! the parameter boundary, before any fetch.
//!
//! **Coexistence with the SSRF channel** (`ssrf.rs`, same issue) needs no test
//! here: both shapes put the slug under the same `data.reason` key, so a
//! reader handles both, and each half is already pinned end-to-end by its own
//! suite (`mcp_ssrf_error_class_test.rs` for SSRF, this file for validation).
//! The one asymmetry a reader must tolerate is that the SSRF channel emits
//! `data` WITHOUT a `field` — which is why `error_field` and `error_reason`
//! are separate helpers here and neither requires the other.
//!
//! Run with: cargo nextest run --test mcp_validation_reason_code_test --features mcp

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use wreq::Client;

mod common;
use common::{call_tool, init_session, is_tool_error, tool_text};

/// `validation::MAX_BLOB_LEN` (1_048_576) + 1 — one byte over the cap, so the
/// rejection is the cap and nothing else.
const MAX_BLOB_LEN_PLUS_1: usize = 1_048_577;

/// `error.data` of a JSON-RPC error response, or a panic naming the whole
/// response. The `data` member is where EC-08's contract lives, so a missing
/// one is a finding, not a skip.
fn error_data(resp: &Value) -> &Value {
    resp.get("error")
        .and_then(|e| e.get("data"))
        .unwrap_or_else(|| panic!("expected a JSON-RPC `error.data`, got: {resp}"))
}

/// The `data.field` string — the information the pre-EC-08 bare-string `data`
/// carried. Kept by EC-08 so nothing is lost in the restructure.
fn error_field(resp: &Value) -> String {
    error_data(resp)
        .get("field")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("`error.data.field` must be a string, got: {resp}"))
        .to_string()
}

/// The `data.reason` slug — the machine-readable half. `None` when the key is
/// absent, which is a legitimate state for callers outside `validation.rs`
/// (see `reason_less_invalid_params_keeps_working_for_out_of_module_callers`).
fn error_reason(resp: &Value) -> Option<String> {
    error_data(resp)
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Parse a `validate_url` body — the tool's JSON payload, minus the provenance
/// envelope helper already inside `tool_text`.
fn validate_url_body(resp: &Value) -> Value {
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("validate_url must return a tool result, got: {resp}"));
    let text = tool_text(result);
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("validate_url body must be JSON ({e}), got: {text}"))
}

// ============================================================================
// Channel A — the JSON-RPC rejection carries a stable reason slug
// ============================================================================

/// A `file://` URL in a raw-string parameter is the transport-reachable proof
/// that the slug reaches the wire, not just the Rust value.
///
/// EC-08 on the `url` field itself is covered as a unit test in
/// `validation.rs` rather than here, and that is a property of the typed
/// boundary, not a gap in this suite: since #1116 every tool's `url` argument
/// is an `McpUrl` newtype, so a non-http(s) URL fails DESERIALIZATION
/// (rmcp's `into_tool_argument_error` → `CallToolResult::error`, `isError:true`)
/// and never reaches `require_http_url`. `params_rejection_test.rs` already
/// pins that envelope. `is_internal_link.seed_domain` is a plain `String`, so
/// it is the only URL-shaped argument that still travels the Channel-A path —
/// and it travels it through the very same `require_*` funnel.
#[tokio::test]
async fn unsupported_scheme_rejection_carries_field_and_reason() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "is_internal_link",
        json!({
            "url": "https://example.com/x",
            "seed_domain": "ftp://example.com"
        }),
    )
    .await;

    assert_eq!(
        error_field(&resp),
        "seed_domain",
        "the field tag must name the offending argument, got: {resp}"
    );
    assert_eq!(
        error_reason(&resp).as_deref(),
        Some("unsupported_scheme"),
        "a non-http(s) seed must carry `unsupported_scheme`, got: {resp}"
    );
    // The message is still there for humans; the slug is the ADDITION, not a
    // replacement for it.
    assert!(
        resp.get("error")
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .is_some_and(|m| m.contains("ftp")),
        "the human-readable message must survive, got: {resp}"
    );
}

/// An oversize blob rejection carries `too_long` — the second half of "an
/// agent can tell what to DO (shorten it)".
#[tokio::test]
async fn oversize_blob_rejection_carries_too_long() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "clean_html",
        json!({ "html": "a".repeat(MAX_BLOB_LEN_PLUS_1) }),
    )
    .await;

    assert_eq!(
        error_field(&resp),
        "html",
        "the field tag must name the offending argument, got: {resp}"
    );
    assert_eq!(
        error_reason(&resp).as_deref(),
        Some("too_long"),
        "an oversize blob must carry `too_long`, got: {resp}"
    );
}

/// A numeric bound carries `out_of_range` (clamp it).
///
/// The bound that exercises the `require_*` funnel — and not an inline
/// envelope built in `params.rs`. `crawl_site`'s `max_depth` / `max_pages`
/// are checked by `validate_max_depth` / `validate_max_pages`, which construct
/// their envelope in `params.rs` (`invalid_params(field, msg)`, no slug) —
/// outside this slice's edit surface, so those bounds still answer with a
/// bare `{"field": …}`. That is a known, visible gap, not a silent one: see
/// the report on slice 5. `scrape_batch`'s `concurrency` goes through
/// `require_range_u64`, so it is the bound that carries the slug today.
#[tokio::test]
async fn numeric_bound_rejection_carries_out_of_range() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_batch",
        json!({
            "urls": ["https://example.com"],
            "concurrency": 65
        }),
    )
    .await;

    assert_eq!(
        error_field(&resp),
        "concurrency",
        "the field tag must name the offending argument, got: {resp}"
    );
    assert_eq!(
        error_reason(&resp).as_deref(),
        Some("out_of_range"),
        "a bound violation must carry `out_of_range`, got: {resp}"
    );
}

// ============================================================================
// Channel B — `validate_url`'s success-shaped diagnostic body (EC-04)
// ============================================================================

/// The contract an agent author reads: the tool ALWAYS succeeds, so the
/// `valid` boolean is the only thing that decides the answer.
#[tokio::test]
async fn validate_url_non_http_scheme_is_a_success_shaped_diagnostic() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "validate_url",
        json!({ "url": "ftp://example.com/file" }),
    )
    .await;

    assert!(
        resp.get("error").is_none(),
        "validate_url must never answer with a protocol error, got: {resp}"
    );
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("validate_url must return a tool result, got: {resp}"));
    assert!(
        !is_tool_error(result),
        "a non-http scheme is a DIAGNOSIS, not a tool error: {resp}"
    );

    let body = validate_url_body(&resp);
    assert_eq!(
        body.get("valid").and_then(Value::as_bool),
        Some(false),
        "an ftp URL must report valid:false: {body}"
    );

    let reason = body
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the body must carry a human-readable reason: {body}"));
    assert!(
        !reason.contains("-32602"),
        "`reason` is a human-facing field; the JSON-RPC code must not leak into \
         it (it used to, via ErrorData's Display): {reason}"
    );
    assert!(
        !reason.contains('{'),
        "`reason` must be prose, not a serialized `data` payload: {reason}"
    );
    assert!(
        reason.contains("ftp"),
        "the reason must still name the offending scheme: {reason}"
    );

    assert_eq!(
        body.get("reason_code").and_then(Value::as_str),
        Some("unsupported_scheme"),
        "the stable slug is its own field: {body}"
    );
}

/// A URL that fails to PARSE reports `malformed`, not `unsupported_scheme` —
/// the two are different remedies (fix the shape vs. change the scheme), which
/// is exactly what the coarse taxonomy exists to tell apart.
#[tokio::test]
async fn validate_url_unparseable_reports_malformed() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "validate_url",
        json!({ "url": "not a url" }),
    )
    .await;

    let body = validate_url_body(&resp);
    assert_eq!(body.get("valid").and_then(Value::as_bool), Some(false));
    assert_eq!(
        body.get("reason_code").and_then(Value::as_str),
        Some("malformed"),
        "an unparseable URL must carry `malformed`: {body}"
    );
    let reason = body
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !reason.contains("-32602"),
        "the JSON-RPC code must not leak into `reason`: {reason}"
    );
}

/// The accepted side of the same channel is UNCHANGED — same six fields, no
/// `reason`, no `reason_code`. EC-04 changed the failure body; a caller that
/// reads `scheme`/`host`/`port`/`path`/`query` on success must see exactly what
/// it saw before, and an exact field-set assertion is what pins that (a
/// contains-check would not notice a field being REMOVED).
#[tokio::test]
async fn validate_url_valid_body_keeps_its_exact_field_set() {
    let (base_url, _handle) = common::start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "validate_url",
        json!({ "url": "https://example.com:8443/docs/page?q=1" }),
    )
    .await;

    assert!(
        resp.get("error").is_none(),
        "a valid URL must not error, got: {resp}"
    );
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("validate_url must return a tool result, got: {resp}"));
    assert!(!is_tool_error(result), "valid URL must not be a tool error");

    let body = validate_url_body(&resp);
    let mut fields: Vec<&str> = body
        .as_object()
        .expect("the valid body is a JSON object")
        .keys()
        .map(String::as_str)
        .collect();
    fields.sort_unstable();
    assert_eq!(
        fields,
        ["host", "path", "port", "query", "scheme", "valid"],
        "the valid: true field set is a frozen contract: {body}"
    );
    assert_eq!(
        body.get("valid").and_then(Value::as_bool),
        Some(true),
        "a valid URL must report valid:true: {body}"
    );
    assert_eq!(
        body.get("host").and_then(Value::as_str),
        Some("example.com")
    );
    assert_eq!(body.get("port").and_then(Value::as_u64), Some(8443));
    assert_eq!(body.get("path").and_then(Value::as_str), Some("/docs/page"));
    assert_eq!(body.get("query").and_then(Value::as_str), Some("q=1"));
}
