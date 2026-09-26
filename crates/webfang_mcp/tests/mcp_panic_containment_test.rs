//! MCP panic containment — a panicking tool is an error, not a dead transport
//! (issue #1611, slice F2).
//!
//! The failure this suite exists for is transport-shaped, so it is proven at
//! the transport level, on the REAL middleware stack, through the REAL
//! `StreamableHttpService`:
//!
//! rmcp's `StreamableHttpService::spawn_session_worker`
//! (`transport/streamable_http_server/tower.rs:665`) runs the MCP handler
//! inside `tokio::spawn`, and rmcp 1.8.0 contains no `catch_unwind` anywhere
//! (`grep -rn catch_unwind rmcp-1.8.0/src` → no hits). A panic raised inside a
//! tool therefore never unwinds the HTTP request future: it unwinds the
//! session-worker task, `svc.waiting()` is dropped, the session closes, and
//! the client is left holding a transport that answers nothing. `CatchPanicLayer`
//! on the router cannot see that panic — it only wraps the HTTP request path —
//! which is why the containment lives in `McpHandler::call_tool`.
//!
//! What the suite pins:
//!
//! - a panicking `tools/call` answers HTTP 200 with a JSON-RPC **result**
//!   carrying `isError: true` (not a dropped stream, not a protocol error);
//! - a FOLLOWING `tools/call` on the SAME session id still succeeds — the
//!   regression this issue is about;
//! - the panic payload never reaches the client;
//! - the HTTP-layer mapping is deterministic, in-process, and needs no
//!   listener (contract for `CatchPanicLayer::custom`).
//!
//! Run with: `cargo nextest run -p webfang_mcp --features mcp --test mcp_panic_containment_test`

#![cfg(feature = "mcp")]

mod common;
use common::*;

use rmcp::handler::server::tool::{ToolCallContext, ToolRoute};
use rmcp::model::{CallToolResult, JsonObject, Tool};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpService,
};
use rmcp::ErrorData as McpError;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use tower::ServiceExt;
use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::handlers;
use webfang_mcp::mcp_server::server::{build_mcp_router_with_service, ServerOptions};
use webfang_mcp::mcp_server::{McpHandler, McpState};
use wreq::Client;

/// Name of the test-only tool that panics. Underscore-prefixed so it can never
/// collide with a real tool name (rmcp's own name validator warns on bad ones).
const PANIC_TOOL_NAME: &str = "__panic_probe";

/// Marker planted in the panic payload. Every assertion below is a "the client
/// never sees this" check, so the marker must be unmistakable.
const PANIC_MARKER: &str = "webfang-f2-panic-probe-marker";

/// A cheap, network-free production tool used as the control and as the
/// "session still alive" probe.
const CONTROL_TOOL: &str = "validate_url";

/// The exact `Accept` value a spec-compliant MCP client must send (rmcp rejects
/// anything that does not contain BOTH media types).
const MCP_ACCEPT: &str = "application/json, text/event-stream";

/// Deadline for a single `tools/call` round trip.
///
/// The failure this suite guards against — a dead session worker — does NOT
/// answer: the request hangs. Without a bound here, a regression surfaces as a
/// nextest slow-timeout kill of the whole test binary instead of as a named
/// assertion, which is exactly how the pre-fix behavior first showed up.
/// Generous (10s) because it only has to outlast a loopback round trip.
const RESPONSE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

// ---------------------------------------------------------------------------
// The test-only panicking tool
// ---------------------------------------------------------------------------

/// Minimal empty-object JSON schema for the probe tool (it takes no arguments).
fn panic_probe_tool_attribute() -> Tool {
    let mut schema = JsonObject::new();
    schema.insert("type".to_string(), json!("object"));
    schema.insert("properties".to_string(), json!({}));
    Tool::new(
        PANIC_TOOL_NAME,
        "Test-only tool that panics on purpose (issue #1611, slice F2).",
        schema,
    )
}

/// The panicking tool body: an `async` block so the panic is raised while the
/// dispatch future is being **polled**, exactly like a tool that panics after
/// its own `.await` would.
fn panic_probe_route<'a>(
    _context: ToolCallContext<'a, McpHandler>,
) -> Pin<Box<dyn Future<Output = Result<CallToolResult, McpError>> + Send + 'a>> {
    Box::pin(async {
        panic!("{PANIC_MARKER}: deliberate tool panic");
    })
}

/// Production tool router plus the probe, mounted through the documented
/// composition seam so the test exercises the real server.
fn tool_router_with_panic_probe() -> rmcp::handler::server::tool::ToolRouter<McpHandler> {
    handlers::build_tool_router().with_route(ToolRoute::new_dyn(
        panic_probe_tool_attribute(),
        panic_probe_route,
    ))
}

/// Start a server whose handler carries the panicking tool, on the real stack
/// via `build_mcp_router_with_service` (#1611, F2 composition seam).
async fn start_panic_probe_server() -> (String, tokio::task::JoinHandle<()>) {
    // This suite DOES speak HTTP — over a loopback listener, never outbound:
    // the containment is a transport-level property, so the test drives the real
    // one. What it never does is fetch anything external, so the SSRF hatches
    // stay identical to every other starter (process-wide, idempotent — see
    // `arm_wiremock_hatches`).
    arm_wiremock_hatches();

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let tool_router = tool_router_with_panic_probe();

    let service = StreamableHttpService::new(
        move || {
            Ok(McpHandler::with_tool_router(
                state.clone(),
                tool_router.clone(),
            ))
        },
        LocalSessionManager::default().into(),
        Default::default(),
    );

    let app = build_mcp_router_with_service(service, &ServerOptions::default());
    serve_on_random_port(app).await
}

/// POST one `tools/call` on an existing session and return `(status, body)`.
///
/// The status is part of the contract under test (a contained panic must be
/// HTTP 200 with a JSON-RPC result), so the shared `call_tool` helper — which
/// drops the status — is not enough here.
///
/// Bounded by [`RESPONSE_DEADLINE`]: a request that is never answered is a
/// dead transport, and a dead transport must FAIL this test, not hang it.
async fn post_tool_call(
    client: &Client,
    base_url: &str,
    session_id: &str,
    name: &str,
    arguments: Value,
) -> (u16, String) {
    tokio::time::timeout(
        RESPONSE_DEADLINE,
        post_tool_call_unbounded(client, base_url, session_id, name, arguments),
    )
    .await
    .unwrap_or_else(|_| {
        panic!("tools/call {name} was never answered within {RESPONSE_DEADLINE:?} — dead transport")
    })
}

async fn post_tool_call_unbounded(
    client: &Client,
    base_url: &str,
    session_id: &str,
    name: &str,
    arguments: Value,
) -> (u16, String) {
    let response = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .header("mcp-session-id", session_id)
        .json(&mcp_request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        ))
        .send()
        .await
        .expect("tools/call request is sent");
    let status = response.status().as_u16();
    let text = response.text().await.expect("response body reads");
    (status, text)
}

// ===========================================================================
// Control — the harness can talk to this server at all
// ===========================================================================

/// Control: a cheap, network-free tool call answers a non-error result on a
/// live session of the PRODUCTION router.
///
/// Without it, a failure below could be an artifact of a broken handshake or
/// of a mis-mounted seam rather than of panic containment.
#[tokio::test]
async fn control_tool_call_on_live_session_is_not_an_error() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let (status, text) = post_tool_call(
        &client,
        &base_url,
        &session_id,
        CONTROL_TOOL,
        json!({ "url": "https://example.com/page" }),
    )
    .await;

    assert_eq!(status, 200, "a normal tool call must be HTTP 200: {text}");
    let response: Value = extract_json(&text).expect("response parses as JSON-RPC");
    assert!(
        response.get("error").is_none(),
        "control call must not be a protocol error: {response}"
    );
    assert!(
        !is_tool_error(response.get("result").expect("control result present")),
        "control call must not be a tool error: {response}"
    );
}

// ===========================================================================
// The proof — a panicking tool is contained and the session survives
// ===========================================================================

/// The probe tool really is mounted: `tools/list` advertises it, so a
/// `-32602 invalid tool name` cannot masquerade as successful containment.
#[tokio::test]
async fn panic_probe_tool_is_advertised_on_the_real_stack() {
    let (base_url, _handle) = start_panic_probe_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let (status, text) = post_tool_call(
        &client,
        &base_url,
        &session_id,
        CONTROL_TOOL,
        json!({ "url": "https://example.com/" }),
    )
    .await;
    assert_eq!(status, 200, "sanity call must succeed: {text}");

    let response = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&mcp_request("tools/list", json!({})))
        .send()
        .await
        .expect("tools/list is sent");
    let listed = extract_json(&response.text().await.expect("body reads"))
        .expect("tools/list parses as JSON-RPC");
    let tools = listed
        .get("result")
        .and_then(|r| r.get("tools"))
        .and_then(Value::as_array)
        .expect("tools/list result carries a tools array");
    assert!(
        tools
            .iter()
            .any(|t| t.get("name").and_then(Value::as_str) == Some(PANIC_TOOL_NAME)),
        "the probe tool must be reachable on the real stack, got: {listed}"
    );
}

/// A panicking `tools/call` answers HTTP 200 with a JSON-RPC result carrying
/// `isError: true` — a normal tool error, not a dropped stream, not a
/// JSON-RPC error envelope, and with the panic payload withheld.
///
/// Bounded on purpose: a session that did NOT survive would leave the request
/// hanging rather than failing, so without this deadline the regression reports
/// itself as a nextest slow-timeout kill (which is how the pre-fix behavior was
/// observed) instead of as a named assertion failure.
#[tokio::test]
async fn panicking_tool_call_answers_200_with_is_error_and_no_payload() {
    let (base_url, _handle) = start_panic_probe_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let (status, text) =
        post_tool_call(&client, &base_url, &session_id, PANIC_TOOL_NAME, json!({})).await;

    assert_eq!(
        status, 200,
        "a contained panic must be HTTP 200 with a JSON-RPC result, got {status}: {text}"
    );
    let response: Value = extract_json(&text).expect("response parses as JSON-RPC");
    assert!(
        response.get("error").is_none(),
        "containment produces a tool RESULT, not a protocol error: {response}"
    );
    let result = response
        .get("result")
        .expect("a contained panic must still answer a JSON-RPC result");
    assert!(
        is_tool_error(result),
        "the contained panic must be flagged isError: {response}"
    );

    let message = tool_text(result);
    assert!(
        !message.is_empty(),
        "the client must get an explanation, not an empty body: {response}"
    );
    assert!(
        !text.contains(PANIC_MARKER),
        "the panic payload must never reach the client: {text}"
    );
    assert!(
        !message.contains("panicked") && !message.contains("panic"),
        "the user-facing message must be a plain explanation, not the panic text: {message}"
    );
}

/// THE regression: after a panicking call, a FOLLOWING `tools/call` on the
/// SAME session id still succeeds.
///
/// Before the fix, the panic unwound rmcp's session worker, the session died,
/// and this call hung or errored — which is the whole point of the issue.
#[tokio::test]
async fn session_survives_a_panicking_tool_call() {
    let (base_url, _handle) = start_panic_probe_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let (panic_status, panic_text) =
        post_tool_call(&client, &base_url, &session_id, PANIC_TOOL_NAME, json!({})).await;
    assert_eq!(
        panic_status, 200,
        "the panicking call must be contained: {panic_text}"
    );

    // Same session id, no re-initialization: the session worker is still there.
    let (status, text) = post_tool_call(
        &client,
        &base_url,
        &session_id,
        CONTROL_TOOL,
        json!({ "url": "https://example.com/after-panic" }),
    )
    .await;

    assert_eq!(
        status, 200,
        "a call on the SAME session after a panic must be HTTP 200, got {status}: {text}"
    );
    let response: Value = extract_json(&text).expect("post-panic response parses as JSON-RPC");
    assert!(
        response.get("error").is_none(),
        "the session must still be dispatching: {response}"
    );
    let result = response
        .get("result")
        .expect("post-panic call must answer a result");
    assert!(
        !is_tool_error(result),
        "the session worker must be fully usable again, not degraded: {response}"
    );
    let payload = tool_text(result);
    assert!(
        payload.contains("after-panic"),
        "the surviving call must return its real answer, got: {response}"
    );
}

// ===========================================================================
// Contract — the HTTP-layer mapping (no listener, no network, deterministic)
// ===========================================================================

/// The outermost `CatchPanicLayer` turns a panic on the HTTP request path into
/// a JSON-RPC `-32603` document over HTTP 500, instead of tower-http's default
/// EMPTY 500 (which an agent reads as a dead transport).
///
/// Driven with `tower::ServiceExt::oneshot` against the REAL stack built by
/// `build_mcp_router_with_service`, so this pins the production mapping and not
/// a replica of it. The panicking service stands in for the `/mcp` service.
#[tokio::test]
async fn panic_on_the_http_path_maps_to_a_jsonrpc_error_body() {
    async fn panicking_route(
        _request: axum::http::Request<axum::body::Body>,
    ) -> Result<axum::response::Response, Infallible> {
        panic!("{PANIC_MARKER}: deliberate HTTP-path panic");
    }

    let app = build_mcp_router_with_service(
        tower::service_fn(panicking_route),
        &ServerOptions::default(),
    );
    let request = axum::http::Request::builder()
        .uri("/mcp")
        .body(axum::body::Body::empty())
        .expect("valid request");

    let response = app
        .oneshot(request)
        .await
        .expect("CatchPanicLayer turns the panic into a response, not a dead service");

    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "a contained HTTP-path panic must be a 500, not a dropped connection"
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/json"),
        "the contained-panic body must be JSON, so the client can parse it"
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let text = String::from_utf8_lossy(&body).to_string();
    let parsed: Value = serde_json::from_str(&text).expect("mapped body is JSON");
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert!(
        parsed["id"].is_null(),
        "a panic on the request path has no request id to echo: {parsed}"
    );
    assert_eq!(
        parsed["error"]["code"], -32603,
        "must be JSON-RPC Internal error: {parsed}"
    );
    assert!(
        !parsed["error"]["message"]
            .as_str()
            .expect("error message is a string")
            .is_empty(),
        "the client must get an explanation: {parsed}"
    );
    assert!(
        !text.contains(PANIC_MARKER),
        "the panic payload must never reach the client: {text}"
    );
}

/// Triangulation: a well-behaved service on the same seam is NOT rewritten
/// into an error body. Without this, a stack that mapped EVERY response to
/// `-32603` would satisfy the test above.
#[tokio::test]
async fn non_panicking_service_passes_through_unchanged() {
    async fn ok_route(
        _request: axum::http::Request<axum::body::Body>,
    ) -> Result<axum::response::Response, Infallible> {
        Ok(axum::response::Response::new(axum::body::Body::empty()))
    }

    let app = build_mcp_router_with_service(tower::service_fn(ok_route), &ServerOptions::default());
    let request = axum::http::Request::builder()
        .uri("/mcp")
        .body(axum::body::Body::empty())
        .expect("valid request");

    let response = app.oneshot(request).await.expect("service answers");
    assert_eq!(
        response.status(),
        axum::http::StatusCode::OK,
        "a non-panicking request must pass through the containment layer untouched"
    );
}
