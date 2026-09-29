//! MCP session admission cap — #1611, F6.
//!
//! `LocalSessionManager` has no count knob of any kind, so before this slice an
//! HTTP client could open sessions without bound, each one costing a spawned
//! worker, two channels and a cloned `McpHandler`. The cap is an axum
//! middleware that charges every request rmcp would turn into a NEW session and
//! sheds the surplus with 429; `server.rs::SessionCap` documents the design and
//! the three consequences it deliberately accepts.
//!
//! Every test here drives the REAL router through a real listener (the only
//! place the middleware stack and rmcp's own response headers are observable
//! together), with a cap small enough to reach in three requests. Nothing in
//! this file sleeps or polls: the cap is configured to a 300s window, so the
//! counter state is a pure function of the request sequence.
//!
//! Run: `cargo nextest run -p webfang_mcp --features mcp --test mcp_session_cap_test`

#![cfg(feature = "mcp")]

mod common;
use common::{extract_json, mcp_request, serve_on_random_port};

use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use serde_json::{json, Value};
use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::{build_mcp_router, ServerOptions};
use webfang_mcp::mcp_server::state::McpState;
use wreq::Client;

/// The `Accept` value a spec-compliant MCP client must send — rmcp rejects
/// anything that does not name both media types (`tower.rs`), so a probe that
/// omits it would be testing the 406 gate instead of the cap.
const MCP_ACCEPT: &str = "application/json, text/event-stream";

/// Window used by every test that is not about release: long enough that the
/// counter is a pure function of the request sequence, so no test can pass or
/// fail on a clock. The release tests use a 1s window instead.
const MAX_WINDOW: Duration = Duration::from_secs(300);

/// Start the real router with an explicit admission cap.
///
/// The cap knobs are the point of these tests, and the shared harness
/// (`common::start_test_server`) builds `ServerOptions::default()` — 64 slots
/// cannot be reached in a test — so the bootstrap is spelled out here. It is
/// the harness's own sequence (container → `McpState` → `build_mcp_router` →
/// `serve_on_random_port`) with the cap threaded through.
async fn start_capped_server(
    max_sessions: usize,
    window_secs: u64,
) -> (String, tokio::task::JoinHandle<()>) {
    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);

    let options = ServerOptions {
        max_sessions: NonZeroUsize::new(max_sessions).expect("test cap is non-zero"),
        session_cap_window_secs: NonZeroU64::new(window_secs).expect("test window is non-zero"),
        ..Default::default()
    };

    let app = build_mcp_router(state, &options);
    serve_on_random_port(app).await
}

/// A well-formed `initialize` body — the request that creates a session.
fn initialize_body() -> Value {
    mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "session-cap-test", "version": "1.0.0" }
        }),
    )
}

/// POST a session-less body to `/mcp` (no `mcp-session-id`), returning
/// `(status, session-id header)`.
async fn post_sessionless(client: &Client, base_url: &str, body: &Value) -> (u16, Option<String>) {
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .json(body)
        .send()
        .await
        .expect("request should be sent");
    let status = resp.status().as_u16();
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    (status, session)
}

/// Initialize one session, returning its id.
async fn open_session(client: &Client, base_url: &str) -> String {
    let (status, session) = post_sessionless(client, base_url, &initialize_body()).await;
    assert_eq!(
        status, 200,
        "initialize under the cap must succeed, got {status}"
    );
    session.expect("an admitted initialize returns mcp-session-id")
}

/// POST on an ESTABLISHED session — traffic the cap must never charge.
async fn post_on_session(
    client: &Client,
    base_url: &str,
    session_id: &str,
    body: &Value,
) -> (u16, String) {
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .header("mcp-session-id", session_id)
        .json(body)
        .send()
        .await
        .expect("request should be sent");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("body should read");
    (status, text)
}

// ============================================================================
// The cap sheds, and the first `max` are untouched
// ============================================================================

/// The load-bearing property: the first `max_sessions` session-creating
/// requests are served exactly as before the cap existed — HTTP 200, a
/// DISTINCT `mcp-session-id` each — and the next one is shed 429.
///
/// The two halves matter separately. "Served 200" alone would pass if the cap
/// silently collapsed every initialize onto one session; the distinct ids are
/// what prove the admitted requests are real, separate sessions.
#[tokio::test]
async fn initialize_past_the_cap_is_shed_with_429() {
    let (base_url, _handle) = start_capped_server(2, MAX_WINDOW.as_secs()).await;
    let client = Client::new();

    let first = open_session(&client, &base_url).await;
    let second = open_session(&client, &base_url).await;
    assert_ne!(
        first, second,
        "each admitted initialize must create its own session"
    );

    let (status, session) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(
        status, 429,
        "the request past the cap must be shed, got {status}"
    );
    assert!(
        session.is_none(),
        "a shed request must NOT create a session: {session:?}"
    );
}

/// The shed is a bare HTTP answer, not a JSON-RPC envelope — the same shape the
/// rate limiter above it already returns, and deliberately NOT the 500 plain-text
/// that rmcp would produce if this were enforced inside a `SessionManager`.
#[tokio::test]
async fn the_shed_is_a_bare_http_answer_not_a_jsonrpc_envelope() {
    let (base_url, _handle) = start_capped_server(1, MAX_WINDOW.as_secs()).await;
    let client = Client::new();

    let _ = open_session(&client, &base_url).await;

    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .json(&initialize_body())
        .send()
        .await
        .expect("request should be sent");
    assert_eq!(resp.status().as_u16(), 429);
    let text = resp.text().await.expect("body should read");

    assert!(
        !text.contains("jsonrpc"),
        "load shedding is an HTTP-layer answer; a JSON-RPC envelope here would \
         promise a protocol error the transport never produced: {text}"
    );
    assert!(
        !text.contains("500"),
        "a shed admission must not be reported as a server fault (that is what \
         enforcing it inside rmcp's SessionManager would produce): {text}"
    );
}

/// Traffic on an ESTABLISHED session is never charged: the cap must not be
/// able to lock out a client that is simply using the session it already has.
///
/// With one slot and one live session the budget is full, so a second
/// session-creating request must still be shed while unlimited in-session
/// requests keep working — that is the difference between a cap and a
/// self-inflicted outage.
#[tokio::test]
async fn established_session_traffic_is_never_charged_to_the_cap() {
    let (base_url, _handle) = start_capped_server(1, MAX_WINDOW.as_secs()).await;
    let client = Client::new();
    let session_id = open_session(&client, &base_url).await;

    for round in 0..5 {
        let (status, text) = post_on_session(
            &client,
            &base_url,
            &session_id,
            &mcp_request("tools/list", json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "round {round}: in-session traffic must be served while the cap is full, got {status}: {text}"
        );
        let parsed: Value = extract_json(&text).expect("tools/list answers JSON-RPC");
        assert!(
            parsed.get("error").is_none(),
            "round {round}: in-session traffic must not be shed: {parsed}"
        );
    }

    // The slot is still held by the live session: a NEW session is still shed.
    let (status, _) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(
        status, 429,
        "in-session traffic must not have released the session's slot"
    );
}

// ============================================================================
// What the cap is (and is not) charged for
// ============================================================================

/// Off-endpoint POSTs are the cheapest denial-of-service against the cap
/// itself, and they allocate nothing: a scanner must not be able to spend the
/// operator's session budget with requests that a 404 would have answered.
///
/// This is the row that would regress silently if the cap were mounted on the
/// whole router without a path gate, and it is why the test asserts the 404s
/// too — the failure mode is "correct 429, wrong reason".
#[tokio::test]
async fn off_endpoint_posts_consume_no_session_budget() {
    let (base_url, _handle) = start_capped_server(1, MAX_WINDOW.as_secs()).await;
    let client = Client::new();

    for round in 0..3 {
        let resp = client
            .post(format!("{base_url}/not-mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", MCP_ACCEPT)
            .json(&initialize_body())
            .send()
            .await
            .expect("request should be sent");
        let status = resp.status().as_u16();
        assert_eq!(
            status, 404,
            "round {round}: a POST off the MCP endpoint is not a session, so the \
             cap must not answer for it either: {status}"
        );
    }

    // The whole budget is still available.
    let _ = open_session(&client, &base_url).await;
    let (status, _) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(
        status, 429,
        "exactly one session fits: the off-endpoint POSTs spent nothing"
    );
}

/// The cap is charged by rmcp's own allocation trigger, which is broader than a
/// well-formed `initialize`: rmcp creates the session BEFORE it validates the
/// message, so a session-less POST of anything else is answered 422 and still
/// leaves an entry in the session map. A cap that only counted `initialize`
/// would leave that vector uncapped.
///
/// Over-counting is the safe direction (shed load, never admit it), and it
/// never changes a status code the transport already owns: the framework's 422
/// is still what an under-cap client sees.
#[tokio::test]
async fn a_sessionless_non_initialize_post_is_charged_and_still_answered_422() {
    let (base_url, _handle) = start_capped_server(1, MAX_WINDOW.as_secs()).await;
    let client = Client::new();

    // Under the cap: rmcp's own session gate answers, unchanged.
    let (status, session) =
        post_sessionless(&client, &base_url, &mcp_request("tools/list", json!({}))).await;
    assert_eq!(
        status, 422,
        "the framework owns this answer, the cap must not shadow it: {status}"
    );
    assert!(
        session.is_none(),
        "a 422 allocates no usable session: {session:?}"
    );

    // It was charged anyway, so the single slot is spent.
    let (status, _) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(
        status, 429,
        "the session-less POST that rmcp allocates for must be charged to the cap"
    );
}

// ============================================================================
// Release of a slot
// ============================================================================

/// A slot comes back when its own window slides, so the cap is a rate over a
/// window and not a lifetime quota — a server that sheds once must recover on
/// its own, with no restart and no operator action.
///
/// The window is 1s and the probe sleeps past it. A monotonic clock cannot run
/// backwards and the sleep can only overshoot, which is the only direction that
/// keeps this assertion true, so the test cannot flake.
#[tokio::test]
async fn a_shed_cap_recovers_by_itself_when_the_window_slides() {
    let (base_url, _handle) = start_capped_server(1, 1).await;
    let client = Client::new();

    let session = open_session(&client, &base_url).await;

    let (status, _) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(status, 429, "the single slot is taken");

    tokio::time::sleep(Duration::from_millis(1_100)).await;

    let (status, session_after) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(
        status, 200,
        "the expired admission must have released its slot: {status}"
    );
    let session_after = session_after.expect("a re-admitted initialize returns a session id");
    assert_ne!(
        session, session_after,
        "the recovered slot must carry a NEW session, not revive the old one"
    );
}
