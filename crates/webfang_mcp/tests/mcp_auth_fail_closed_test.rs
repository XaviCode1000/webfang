//! MCP authentication is fail-closed by default — #1611, G-18.
//!
//! The finding was that `expected_token: None` passed EVERY request, so the
//! shipped configuration (loopback bind, no token) meant "anyone who can open a
//! socket may use every scraper tool". That is a fail-open default for a
//! security boundary, and this suite pins the fix: an unset credential is now a
//! REFUSAL, and anonymous operation is an opt-in a developer has to ask for.
//!
//! The rows that must not regress:
//!
//! 1. the default refuses everything, including a well-formed `initialize`;
//! 2. the refusal is a bare `401` with no session allocated;
//! 3. the opt-in restores token-less operation (the development mode);
//! 4. a configured token is required even when the opt-in is also set —
//!    "anonymous allowed" must never mean "the configured token is optional";
//! 5. a session opened while authenticated keeps working when the client
//!    presents its token on every request, and stops working when it does not.
//!
//! Every test drives the REAL router through a real listener: auth is a layer
//! in a mounted stack, and only a mounted stack has an order.
//!
//! Run: `cargo nextest run -p webfang_mcp --features mcp --test mcp_auth_fail_closed_test`

#![cfg(feature = "mcp")]

mod common;
use common::{initialize_body, mcp_request, post_mcp, start_server_with_options, MCP_ACCEPT};

use serde_json::json;
use webfang_mcp::mcp_server::server::ServerOptions;
use wreq::Client;

const TOKEN: &str = "g18-integration-token";

/// The shipped default, spelled out rather than inherited: this is exactly what
/// an operator gets for running the binary with no flags.
async fn start_default_server() -> (String, tokio::task::JoinHandle<()>) {
    start_server_with_options(ServerOptions::default()).await
}

/// The finding's regression row: with no token configured and no opt-in, a
/// well-formed `initialize` is REFUSED. It used to be served — that single
/// status change is the fix.
#[tokio::test]
async fn the_default_configuration_refuses_every_request() {
    let (base_url, _handle) = start_default_server().await;
    let client = Client::new();

    let (status, _, session) =
        post_mcp(&client, &base_url, &initialize_body("auth-test"), None).await;
    assert_eq!(
        status, 401,
        "a token-less default must refuse, not serve: {status}"
    );
    assert!(
        session.is_none(),
        "a refused request must not allocate a session: {session:?}"
    );

    // Presenting an arbitrary credential changes nothing: there is nothing
    // configured to compare it against.
    let (status, _, _) = post_mcp(
        &client,
        &base_url,
        &initialize_body("auth-test"),
        Some("guessed-token"),
    )
    .await;
    assert_eq!(status, 401, "an invented credential buys nothing");
}

/// The development mode still exists — it is now something a developer asks for
/// rather than something they get by configuring nothing.
#[tokio::test]
async fn the_explicit_opt_in_restores_token_less_operation() {
    let (base_url, _handle) = start_server_with_options(ServerOptions {
        allow_anonymous: true,
        ..Default::default()
    })
    .await;
    let client = Client::new();

    let (status, _, session) =
        post_mcp(&client, &base_url, &initialize_body("auth-test"), None).await;
    assert_eq!(status, 200, "the opted-in development mode must serve");
    assert!(session.is_some(), "and it must be a real session");
}

/// The opt-in is ignored while a token is configured: a deployment that set a
/// token did not ask for anonymous access, and "anonymous allowed" must never
/// degrade into "the configured token is optional".
#[tokio::test]
async fn a_configured_token_is_required_even_with_the_opt_in() {
    let (base_url, _handle) = start_server_with_options(ServerOptions {
        auth_token: Some(TOKEN.to_string()),
        allow_anonymous: true,
        ..Default::default()
    })
    .await;
    let client = Client::new();

    let (status, _, _) = post_mcp(&client, &base_url, &initialize_body("auth-test"), None).await;
    assert_eq!(status, 401, "no header, no service — opt-in or not");

    let (status, _, session) = post_mcp(
        &client,
        &base_url,
        &initialize_body("auth-test"),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, 200, "the configured token still works");
    assert!(session.is_some());
}

/// Authentication is per REQUEST, not per session: a client that opened a
/// session with its token still loses access the moment it stops presenting
/// one. Pinned because the alternative reading — "the session is the
/// credential" — is the one a caller would silently assume.
#[tokio::test]
async fn an_established_session_still_needs_the_token_on_every_request() {
    let (base_url, _handle) = start_server_with_options(ServerOptions {
        auth_token: Some(TOKEN.to_string()),
        ..Default::default()
    })
    .await;
    let client = Client::new();

    let (status, _, session) = post_mcp(
        &client,
        &base_url,
        &initialize_body("auth-test"),
        Some(TOKEN),
    )
    .await;
    assert_eq!(status, 200);
    let session_id = session.expect("an authenticated initialize returns a session");

    let body = mcp_request("tools/list", json!({}));
    let authenticated = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .header("mcp-session-id", &session_id)
        .header("Authorization", format!("Bearer {TOKEN}"))
        .json(&body)
        .send()
        .await
        .expect("request should be sent");
    assert_eq!(authenticated.status().as_u16(), 200);

    let anonymous = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&body)
        .send()
        .await
        .expect("request should be sent");
    assert_eq!(
        anonymous.status().as_u16(),
        401,
        "the session id is not a credential"
    );
}
