//! MCP transport contract — what rmcp owns and what we own (issue #1294, P5-1 + P5-2).
//!
//! Root cause of both reported items: the Streamable HTTP transport answers
//! protocol-shape problems at the **HTTP** layer, before any JSON-RPC dispatch.
//! This suite pins exactly which of those answers belong to `rmcp 1.8.0` so the
//! boundary stops being re-litigated, and closes the two items with evidence
//! instead of a code change.
//!
//! Disposition (design approved by the orchestrator, no translation middleware):
//!
//! - **P5-1** — the reported `422` for an unknown JSON-RPC method is NOT a
//!   missing method-not-found implementation: it is rmcp's stateful session gate
//!   (`tower.rs:1149/1160` → `server_side_http.rs:164-169`), which fires for any
//!   session-less POST whose message is not `initialize`. The probe in the issue
//!   never handshook. Inside an established session the same method DOES produce
//!   `-32601` (`handler/server.rs:329`; our `McpHandler` does not override
//!   `on_custom_request`). [`unknown_method_on_live_session_is_jsonrpc_32601`]
//!   proves it.
//! - **P5-2** — a JSON-RPC 1.0 body is answered `415` because rmcp maps **every**
//!   body-deserialization failure to `UNSUPPORTED_MEDIA_TYPE`
//!   (`server_side_http.rs:170-186`), including the missing `jsonrpc:"2.0"`
//!   discriminator. Framework-owned; pinned, not fixed.
//!
//! Every test here is a **characterization** test: it passes against the current
//! build and documents the framework boundary. The webfang-side defect it exposes
//! lives in `mcp_behavioral_test.rs::test_unknown_method_returns_error`, whose
//! `if status.is_success()` escape hatch lets a session-less 422 pass as
//! "acceptable" forever — tightened in this same slice.
//!
//! Run with: `cargo nextest run -p webfang_mcp --features mcp --test mcp_transport_contract_test`

#![cfg(feature = "mcp")]

mod common;
use common::*;

use serde_json::{json, Value};
use wreq::Client;

/// JSON-RPC standard error code for "Method not found" (JSON-RPC 2.0 spec).
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;

/// The exact `Accept` value a spec-compliant MCP client must send: rmcp rejects
/// anything that does not contain BOTH media types (`tower.rs:1018-1031`).
const MCP_ACCEPT: &str = "application/json, text/event-stream";

/// POST one JSON-RPC body and return `(status, raw body text)`.
///
/// `session_id` is omitted for the session-less probes — that omission is the
/// whole point of two of them, so it must be visible at the call site.
async fn post_rpc(
    client: &Client,
    base_url: &str,
    session_id: Option<&str>,
    body: &Value,
) -> (u16, String) {
    let mut req = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT);
    if let Some(session_id) = session_id {
        req = req.header("mcp-session-id", session_id);
    }
    let resp = req
        .json(body)
        .send()
        .await
        .expect("transport-level request should be sent");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("response body should read");
    (status, text)
}

/// Start a server and complete the MCP handshake, returning the session id.
///
/// Every "framework owns this answer" claim below is only meaningful once the
/// session gate is passed, so the handshake is the suite's shared prologue.
async fn live_session() -> (String, Client, String, tokio::task::JoinHandle<()>) {
    let (base_url, handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;
    (base_url, client, session_id, handle)
}

// ============================================================================
// Control: the handshake really works in this binary
// ============================================================================

/// Control probe: `tools/list` on a live session returns a JSON-RPC **result**.
///
/// Without this, a `-32601`/`422` elsewhere in the suite could be an artifact of a
/// broken handshake rather than a contract. It keeps the framework-ownership
/// verdicts falsifiable.
#[tokio::test]
async fn control_tools_list_on_live_session_returns_result() {
    let (base_url, client, session_id, _handle) = live_session().await;

    let (status, text) = post_rpc(
        &client,
        &base_url,
        Some(&session_id),
        &mcp_request("tools/list", json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "tools/list on a live session must be HTTP 200, got: {text}"
    );

    let resp: Value = extract_json(&text).expect("tools/list must parse as JSON-RPC");
    assert!(
        resp.get("error").is_none(),
        "tools/list must not be an error, got: {resp}"
    );
    let tools = resp
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .expect("tools/list result must carry a tools array");
    assert!(
        tools.len() >= 35,
        "expected the full tool surface (>= 35), got {}",
        tools.len()
    );
}

// ============================================================================
// P5-1 — unknown method: -32601 in a session, 422 without one
// ============================================================================

/// P5-1 evidence: inside an established session an unknown JSON-RPC method is a
/// proper `-32601` protocol error over HTTP 200.
///
/// This is rmcp's own default `on_custom_request` (`handler/server.rs:329`);
/// webfang supplies no method-dispatch layer of its own, so there is nothing to
/// fix here — only a contract to pin.
#[tokio::test]
async fn unknown_method_on_live_session_is_jsonrpc_32601() {
    let (base_url, client, session_id, _handle) = live_session().await;

    let (status, text) = post_rpc(
        &client,
        &base_url,
        Some(&session_id),
        &mcp_request("webfang/definitelyNotAMethod", json!({})),
    )
    .await;

    assert_eq!(
        status, 200,
        "a dispatched unknown method must be HTTP 200 with a JSON-RPC error, got {status}: {text}"
    );
    let resp: Value = extract_json(&text).expect("response must parse as JSON-RPC");
    let code = resp
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("expected an error object, got: {resp}"));
    assert_eq!(
        code, JSONRPC_METHOD_NOT_FOUND,
        "unknown method must map to -32601 (Method not found), got: {resp}"
    );
}

/// P5-1 explanation of the reported symptom: the very same method, POSTed
/// **without** a session id, is answered `422` with plain text by rmcp's stateful
/// session gate — the request never reaches JSON-RPC at all.
///
/// This is the case the issue's probe exercised. It agrees with the contract
/// already pinned in `mcp_lifecycle_test.rs::test_no_session_id_returns_422`, so
/// the two suites must keep moving together.
#[tokio::test]
async fn sessionless_unknown_method_is_framework_422_before_jsonrpc() {
    let (base_url, client, _session_id, _handle) = live_session().await;

    let (status, text) = post_rpc(
        &client,
        &base_url,
        None,
        &mcp_request("webfang/definitelyNotAMethod", json!({})),
    )
    .await;

    assert_eq!(
        status, 422,
        "a session-less non-initialize POST must hit rmcp's session gate (422), got {status}: {text}"
    );
    assert!(
        !text.contains("-32601"),
        "the session gate answers before JSON-RPC dispatch, so no protocol error \
         object may appear: {text}"
    );
    assert!(
        text.contains("initialize"),
        "the 422 must name what the client actually got wrong (no session), got: {text}"
    );
}

// ============================================================================
// P5-2 — protocol-shape answers that belong to rmcp
// ============================================================================

/// P5-2 evidence: a JSON-RPC 1.0 body (no `jsonrpc` discriminator) is answered
/// `415` by rmcp, not a protocol error.
///
/// `expect_json` (`server_side_http.rs:170-186`) funnels **every**
/// deserialization failure — malformed JSON, unknown shape, and the missing
/// `jsonrpc:"2.0"` version alike — into `UNSUPPORTED_MEDIA_TYPE`. 400 would be
/// the semantically better code; it is not ours to change without a translation
/// layer, and the approved design rules that out.
#[tokio::test]
async fn jsonrpc_1_0_body_is_framework_415_deserialize_error() {
    let (base_url, client, _session_id, _handle) = live_session().await;

    // JSON-RPC 1.0: id/method/params and no `jsonrpc` field at all.
    let legacy = json!({ "id": 1, "method": "tools/list", "params": {} });
    let (status, text) = post_rpc(&client, &base_url, None, &legacy).await;

    assert_eq!(
        status, 415,
        "a version-less (1.0) body must surface rmcp's deserialize rejection as 415, got {status}: {text}"
    );
    assert!(
        text.contains("fail to deserialize request body"),
        "the 415 must carry rmcp's own deserialization wording so the ownership is \
         auditable, got: {text}"
    );
}

/// Framework gate, same boundary as P5-2: without an `Accept` naming BOTH media
/// types the request is rejected `406` before anything is parsed
/// (`tower.rs:1018-1031`).
#[tokio::test]
async fn accept_gate_rejects_406_when_both_media_types_are_not_advertised() {
    let (base_url, client, _session_id, _handle) = live_session().await;

    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        // Deliberately no `Accept: application/json, text/event-stream`.
        .header("Accept", "application/json")
        .json(&mcp_request("tools/list", json!({})))
        .send()
        .await
        .expect("request should be sent");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("body should read");

    assert_eq!(
        status, 406,
        "an Accept that omits text/event-stream must be 406, got {status}: {text}"
    );
}

/// Framework gate: a non-`application/json` Content-Type is rejected `415` by the
/// header check itself — a *different* 415 from the deserialize one in
/// [`jsonrpc_1_0_body_is_framework_415_deserialize_error`], which is why the body
/// wording is asserted too.
#[tokio::test]
async fn content_type_gate_rejects_415_when_not_application_json() {
    let (base_url, client, _session_id, _handle) = live_session().await;

    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "text/plain")
        .header("Accept", MCP_ACCEPT)
        .body(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}).to_string())
        .send()
        .await
        .expect("request should be sent");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("body should read");

    assert_eq!(
        status, 415,
        "a non-JSON Content-Type must be 415, got {status}: {text}"
    );
    assert!(
        text.contains("Content-Type must be application/json"),
        "the header-gate 415 must be distinguishable from the deserialize 415, got: {text}"
    );
}
