//! MCP rate limiter — keyed, and inside authentication (#1611, F5).
//!
//! The finding was a `NotKeyed` limiter mounted BEFORE authentication, which
//! made two separate guarantees false at once: every caller shared one quota
//! (so a noisy client spent everyone's budget), and a request with no or a
//! wrong bearer token spent that quota before it was refused — so an
//! unauthenticated flood could lock the legitimate operator out of its own
//! server. `server.rs` documents the fix; the properties that must not
//! regress are these:
//!
//! 1. an unauthenticated flood spends NO quota of the authenticated bucket;
//! 2. an authenticated caller over its own quota is shed `429` WITH a
//!    `Retry-After` it can act on;
//! 3. a shed request never reaches rmcp, so it allocates no session.
//!
//! Every test drives the REAL router through a real listener: the layer ORDER
//! is the thing under test, and only a mounted stack has one. No test sleeps
//! — the quotas are 1rps with a burst of 2, and each assertion is stated as
//! "at least one of these consecutive requests was shed", which is exact in
//! the direction that matters (a slow machine can only add refill, never
//! remove a shed).
//!
//! Run: `cargo nextest run -p webfang_mcp --features mcp --test mcp_rate_limit_keyed_test`

#![cfg(feature = "mcp")]

mod common;
use common::{mcp_request, serve_on_random_port};

use serde_json::{json, Value};
use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::{build_mcp_router, ServerOptions};
use webfang_mcp::mcp_server::state::McpState;
use wreq::Client;

/// The `Accept` value a spec-compliant MCP client must send (rmcp answers 406
/// to anything that names neither media type).
const MCP_ACCEPT: &str = "application/json, text/event-stream";

/// The only token this server accepts; the "noisy" and "quiet" callers below
/// differ by whether they present it.
const TOKEN: &str = "f5-integration-token";

/// Start the real router with authentication on and a 1rps / burst-2 quota.
///
/// The quota is deliberately tiny so the bound is reachable in a couple of
/// requests — a limiter that is never exceeded in the suite is an unverified
/// limiter.
async fn start_server_with_token() -> (String, tokio::task::JoinHandle<()>) {
    let options = ServerOptions {
        auth_token: Some(TOKEN.to_string()),
        rate_per_second: 1,
        rate_burst: 2,
        ..Default::default()
    };
    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let app = build_mcp_router(McpState::new(container), &options);
    serve_on_random_port(app).await
}

/// A well-formed `initialize` body — the request that creates a session.
fn initialize_body() -> Value {
    mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "rate-limit-test", "version": "1.0.0" }
        }),
    )
}

/// One POST to `/mcp`, returning `(status, retry-after, session-id)`.
async fn post(
    client: &Client,
    base_url: &str,
    body: &Value,
    token: Option<&str>,
) -> (u16, Option<String>, Option<String>) {
    let mut request = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let resp = request
        .json(body)
        .send()
        .await
        .expect("request should be sent");
    let status = resp.status().as_u16();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    (status, header("retry-after"), header("mcp-session-id"))
}

/// The load-bearing property of the reorder: a flood of requests carrying NO
/// token is refused by auth and must not spend a single cell of the
/// authenticated budget — otherwise the operator who owns the token is locked
/// out by an attacker who does not.
#[tokio::test]
async fn an_unauthenticated_flood_spends_none_of_the_operators_quota() {
    let (base_url, _handle) = start_server_with_token().await;
    let client = Client::new();

    for _ in 0..5 {
        let (status, _, _) = post(&client, &base_url, &initialize_body(), None).await;
        assert_eq!(status, 401, "an anonymous request is refused, not served");
    }

    let (status, _, session) = post(&client, &base_url, &initialize_body(), Some(TOKEN)).await;
    assert_eq!(
        status, 200,
        "the operator must still be served after an unauthenticated flood"
    );
    assert!(
        session.is_some(),
        "the served request is a real session, not a hollow 200"
    );
}

/// A wrong token is refused exactly like a missing one, and likewise spends
/// nothing: the limiter is behind auth either way.
#[tokio::test]
async fn a_wrong_token_is_refused_without_spending_quota() {
    let (base_url, _handle) = start_server_with_token().await;
    let client = Client::new();

    for _ in 0..5 {
        let (status, _, _) = post(
            &client,
            &base_url,
            &initialize_body(),
            Some("not-the-token"),
        )
        .await;
        assert_eq!(status, 401, "a wrong credential is refused");
    }

    let (status, _, _) = post(&client, &base_url, &initialize_body(), Some(TOKEN)).await;
    assert_eq!(status, 200, "the flood spent nothing");
}

/// The shed is actionable: `429` plus a `Retry-After` in whole seconds. A 429
/// with no recovery signal is indistinguishable from a policy refusal to an
/// agent, and invites the retry loop that keeps it locked out.
#[tokio::test]
async fn an_over_quota_caller_is_shed_with_429_and_retry_after() {
    let (base_url, _handle) = start_server_with_token().await;
    let client = Client::new();

    let mut shed = Vec::new();
    let mut served_with_session = 0usize;
    for _ in 0..6 {
        let (status, retry_after, session) =
            post(&client, &base_url, &initialize_body(), Some(TOKEN)).await;
        match status {
            429 => shed.push(retry_after),
            200 => {
                assert!(
                    session.is_some(),
                    "a served initialize must be a real session"
                );
                served_with_session += 1;
            },
            other => panic!("unexpected status {other}: neither served nor shed"),
        }
    }

    assert!(
        !shed.is_empty(),
        "6 authenticated requests against a burst of 2 must be shed at least once"
    );
    assert!(
        served_with_session <= 2,
        "the burst is 2, so at most two are served before refill: {served_with_session}"
    );
    for retry_after in &shed {
        let raw = retry_after
            .as_deref()
            .unwrap_or_else(|| panic!("a shed must carry Retry-After: {shed:?}"));
        let seconds: u64 = raw
            .parse()
            .unwrap_or_else(|_| panic!("Retry-After must be whole seconds: {raw}"));
        assert!(
            (1..=1_000).contains(&seconds),
            "Retry-After must be a plausible number of seconds, got {seconds}"
        );
    }
}

/// A shed request never reaches rmcp, so it must not allocate a session: the
/// `mcp-session-id` header is the observable proof, and without it a flood
/// would buy sessions for free by overrunning the rate limiter.
#[tokio::test]
async fn a_shed_request_allocates_no_session() {
    let (base_url, _handle) = start_server_with_token().await;
    let client = Client::new();

    let mut saw_shed = false;
    for _ in 0..6 {
        let (status, _, session) = post(&client, &base_url, &initialize_body(), Some(TOKEN)).await;
        if status == 429 {
            saw_shed = true;
            assert!(
                session.is_none(),
                "a shed request must NOT create a session: {session:?}"
            );
        }
    }
    assert!(saw_shed, "the burst of 2 must have been exhausted");
}
