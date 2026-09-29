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
//! together), with a cap small enough to reach in three requests. Apart from the
//! two tests that are explicitly about the passage of time (the release test
//! and the rate-limiter refill, both of which sleep a known lower bound), no
//! test here sleeps or polls: those two use a 1s window / a 1rps quota, so the
//! counter state is otherwise a pure function of the request sequence.
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
    start_server_with_options(ServerOptions {
        max_sessions: NonZeroUsize::new(max_sessions).expect("test cap is non-zero"),
        session_cap_window_secs: NonZeroU64::new(window_secs).expect("test window is non-zero"),
        ..Default::default()
    })
    .await
}

/// The same bootstrap for tests that need a DIFFERENT knob changed — an auth
/// token, a tight rate limiter — while still reaching the cap in a handful of
/// requests.
async fn start_server_with_options(
    options: ServerOptions,
) -> (String, tokio::task::JoinHandle<()>) {
    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);

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
///
/// It is also not silent: a 429 with no recovery signal leaves an agent (or an
/// operator watching a dashboard) with nothing to do but hammer the endpoint, so
/// the shed carries `Retry-After` in seconds. The value is the wait until the
/// OLDEST live admission ages out, which is bounded by the configured window —
/// 1 to 300s here — so the assertion is a range, not an exact number.
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
    let retry_after = resp
        .headers()
        .get("retry-after")
        .expect("a shed must tell the client when to come back")
        .to_str()
        .expect("Retry-After is ASCII")
        .to_string();
    let retry_after: u64 = retry_after
        .parse()
        .unwrap_or_else(|e| panic!("Retry-After must be an integer number of seconds: {e}"));
    assert!(
        (1..=MAX_WINDOW.as_secs()).contains(&retry_after),
        "Retry-After must fit the configured window, got {retry_after}"
    );
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
    assert!(
        text.is_empty(),
        "the shed body stays empty — the header carries the wait and the trace \
         carries the reason: {text}"
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
/// POST a session-less body to `/mcp` with a Bearer token, returning
/// `(status, session-id header)`.
async fn post_sessionless_authorized(
    client: &Client,
    base_url: &str,
    body: &Value,
    token: &str,
) -> (u16, Option<String>) {
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .header("Authorization", format!("Bearer {token}"))
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

// ============================================================================
// The predicate is rmcp's allocation trigger, not an approximation of it
// ============================================================================

/// rmcp reads the session header as
/// `part.headers.get(HEADER_SESSION_ID).and_then(|v| v.to_str().ok())`
/// (`tower.rs:1059-1060`) and allocates in the `else` arm, so a header that is
/// PRESENT but not valid UTF-8 makes rmcp allocate a session. obs-text bytes
/// (0x80-0xFF) are perfectly legal `HeaderValue` bytes, so this is a request
/// any client can send — which is exactly what makes a `contains_key`-shaped
/// predicate a one-byte bypass: unbounded sessions, and never a 429.
///
/// The assertion that matters is the second one. A 429 alone could be a
/// coincidental shed; "no `mcp-session-id` came back" is what proves the
/// charged request never reached `create_session`.
#[tokio::test]
async fn a_non_utf8_session_id_header_is_charged_like_a_missing_one() {
    let (base_url, _handle) = start_capped_server(1, MAX_WINDOW.as_secs()).await;
    let client = Client::new();

    // The single slot is taken by an ordinary session.
    let session = open_session(&client, &base_url).await;
    assert!(!session.is_empty(), "the first session is real");

    let mut headers = wreq::header::HeaderMap::new();
    headers.insert(
        "mcp-session-id",
        wreq::header::HeaderValue::from_bytes(b"\x80").expect("obs-text is a legal header byte"),
    );
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .headers(headers)
        .json(&initialize_body())
        .send()
        .await
        .expect("request should be sent");

    assert_eq!(
        resp.status().as_u16(),
        429,
        "a session id rmcp cannot read must be charged like a missing one"
    );
    let allocated = resp.headers().get("mcp-session-id").cloned();
    assert!(
        allocated.is_none(),
        "a shed request must not allocate a session: {allocated:?}"
    );
}

// ============================================================================
// Layer order: the cap is charged AFTER auth and the rate limiter
// ============================================================================

/// The security property stated in `build_mcp_router_with_service`: the cap is
/// layered behind auth, so a 401 can never consume a session slot. Without
/// that ordering an unauthenticated flood would spend the LEGITIMATE
/// operator's session budget — a cap that doubles as a denial-of-service
/// against the operator, reachable by anyone who can reach the port.
///
/// Shape: one slot, an auth token configured, a flood of well-formed
/// `initialize` bodies that all fail auth, and then the operator's own
/// request. The flood's failure is the point: if any of those 401s had been
/// charged, the operator's request below would be shed instead of served.
#[tokio::test]
async fn unauthenticated_requests_consume_no_session_budget() {
    const TOKEN: &str = "s3cret-token";
    let (base_url, _handle) = start_server_with_options(ServerOptions {
        max_sessions: NonZeroUsize::new(1).expect("one is non-zero"),
        session_cap_window_secs: NonZeroU64::new(MAX_WINDOW.as_secs())
            .expect("test window is non-zero"),
        auth_token: Some(TOKEN.to_string()),
        ..Default::default()
    })
    .await;
    let client = Client::new();

    for round in 0..3 {
        let (status, session) = post_sessionless(&client, &base_url, &initialize_body()).await;
        assert_eq!(
            status, 401,
            "round {round}: the flood must be rejected by auth, not served: {status}"
        );
        assert!(
            session.is_none(),
            "a 401 must not allocate a session: {session:?}"
        );
    }

    // The whole budget is still available to the legitimate client.
    let (status, session) =
        post_sessionless_authorized(&client, &base_url, &initialize_body(), TOKEN).await;
    assert_eq!(
        status, 200,
        "the unauthenticated flood must not have spent the operator's slot: {status}"
    );
    assert!(
        session.is_some(),
        "the operator's initialize must create a real session"
    );

    // ...and exactly one session fits, so the cap itself is still armed.
    let (status, _) =
        post_sessionless_authorized(&client, &base_url, &initialize_body(), TOKEN).await;
    assert_eq!(
        status, 429,
        "the 401s spent nothing, so this second session is what exhausts the cap"
    );
}

/// Same claim for the layer above auth: a request shed 429 by the rate limiter
/// never reaches the cap either, so a burst of traffic cannot burn the session
/// budget on top of being rate limited.
///
/// Quota is 1rps with burst 1, so the first request consumes the burst and the
/// next three are shed by the limiter. Both layers answer 429, so the sheds
/// alone prove nothing — the PROOF is the request after the quota refills: with
/// two slots and only one taken, a served 200 means the shed requests charged
/// nothing. The 1.2s wait is a lower bound on the 1s refill, so it can only
/// overshoot.
#[tokio::test]
async fn rate_limiter_sheds_consume_no_session_budget() {
    let (base_url, _handle) = start_server_with_options(ServerOptions {
        max_sessions: NonZeroUsize::new(2).expect("two is non-zero"),
        session_cap_window_secs: NonZeroU64::new(MAX_WINDOW.as_secs())
            .expect("test window is non-zero"),
        rate_per_second: 1,
        rate_burst: 1,
        ..Default::default()
    })
    .await;
    let client = Client::new();

    // Takes the whole burst and one of the two slots.
    let (status, session) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(status, 200, "the first request fits the burst: {status}");
    assert!(session.is_some(), "and it creates a real session");

    for round in 0..3 {
        let (status, session) = post_sessionless(&client, &base_url, &initialize_body()).await;
        assert_eq!(
            status, 429,
            "round {round}: the burst is spent, so the limiter sheds: {status}"
        );
        assert!(
            session.is_none(),
            "a shed request allocates nothing: {session:?}"
        );
    }

    tokio::time::sleep(Duration::from_millis(1_200)).await;

    let (status, session) = post_sessionless(&client, &base_url, &initialize_body()).await;
    assert_eq!(
        status, 200,
        "the limiter-shed requests must not have spent the second slot: {status}"
    );
    assert!(
        session.is_some(),
        "the re-admitted request creates a real session"
    );
}

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
