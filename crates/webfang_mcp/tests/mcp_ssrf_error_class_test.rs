//! MCP SSRF entry pre-check — error CLASS contract (issue #1613, slice 2, EC-03).
//!
//! `validate_url_no_ssrf` used to collapse five distinct failures into one
//! shape: `-32602 invalid_params` with `data: None`. That made a transient DNS
//! outage indistinguishable from a caller aiming at internal infrastructure,
//! and the only way to tell them apart was to parse Spanish message text. This
//! suite pins the machine-readable half of the replacement decision table —
//! the JSON-RPC `code` plus the `data.reason` slug:
//!
//! | class | code | `data.reason` |
//! | :--- | :--- | :--- |
//! | policy (literal) | `-32602` | `forbidden_ip_literal` |
//! | policy (resolved) | `-32602` | `forbidden_ip_resolved` |
//! | infrastructure (resolver error) | `-32603` | `dns_resolution_failed` |
//! | infrastructure (empty answer set) | `-32603` | `dns_no_addresses` |
//! | caller input (params validation) | `-32602` | field tag, e.g. `"max_depth"` |
//!
//! Because policy and caller input intentionally SHARE `-32602`, the slug is
//! the only thing separating them — every assertion below is on
//! `error.data.reason`, never on message text.
//!
//! Run with: cargo nextest run --test mcp_ssrf_error_class_test --features mcp

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use wreq::Client;

use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::server::ServerOptions;
use webfang_mcp::mcp_server::state::McpState;

// Session/tool-call JSON-RPC helpers live in the shared harness (`tests/common`).
// Imported by name (not `use common::*`) so this file's own `start_test_server`
// cannot be shadowed by — or collide with — `common::start_test_server`.
mod common;
use common::{call_tool, init_session};

/// JSON-RPC "Invalid params".
const JSONRPC_INVALID_PARAMS: i64 = -32602;
/// JSON-RPC "Internal error".
const JSONRPC_INTERNAL_ERROR: i64 = -32603;

/// The two DNS-failure slugs. They are the SAME class (infrastructure) and
/// therefore the same code; whether a given environment lands on the resolver
/// `Err` branch or the empty-answer-set branch depends on the resolver
/// implementation (systemd-resolved answers NXDOMAIN as an error; some stubs
/// answer an empty RRset), so a DNS-infrastructure assertion accepts either.
/// The class assertion — `-32603` — is what this suite actually proves.
const DNS_REASONS: &[&str] = &["dns_resolution_failed", "dns_no_addresses"];

/// RFC 2606 reserved TLD: guaranteed never to resolve, so this fixture is a
/// deterministic infrastructure failure without reaching any real host.
const UNRESOLVABLE_URL: &str = "http://nonexistent-host-1613.invalid/";

// ============================================================================
// Harness — the starter stays local (see its note); the JSON-RPC helpers come
// from `tests/common/mod.rs`.
// ============================================================================

/// Start a test MCP server with the SSRF guard LEFT ON.
///
/// Deliberately NOT `common::start_test_server`: that one disarms the
/// wiremock-loopback SSRF hatches, and this suite exists precisely to observe
/// the guard's refusals — swapping in the disarming starter would silently
/// change what is being asserted (every refusal here would vanish and the tests
/// would pass for the wrong reason). The hatches are actively REMOVED rather
/// than merely left untouched, so an ambient `WEBFANG_MCP_DISABLE_SSRF=1`
/// exported by a shared CI/parent environment cannot disarm the guard either.
async fn start_test_server() -> (String, JoinHandle<()>) {
    use webfang_core::domain::ssrf_guard::{DISABLE_ENTRY_GUARD_ENV, WEBFANG_MCP_DISABLE_SSRF_ENV};

    webfang_test_utils::env_remove(WEBFANG_MCP_DISABLE_SSRF_ENV);
    webfang_test_utils::env_remove(DISABLE_ENTRY_GUARD_ENV);

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let app = build_mcp_router(state, &ServerOptions::default());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Wait for the server to accept TCP connections instead of a fixed sleep.
    for _ in 0..20 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    (base_url, handle)
}

/// The JSON-RPC `error.code`, if the envelope carries a protocol error.
fn error_code(resp: &Value) -> Option<i64> {
    resp.get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_i64)
}

/// The JSON-RPC `error.data.reason` slug, if present.
fn error_reason(resp: &Value) -> Option<&str> {
    resp.get("error")
        .and_then(|e| e.get("data"))
        .and_then(|d| d.get("reason"))
        .and_then(Value::as_str)
}

/// Assert `code` and `reason` together, quoting the whole envelope on failure
/// so a mismatch is diagnosable without re-running with a debugger.
fn assert_class(resp: &Value, code: i64, reason: &str, context: &str) {
    assert_eq!(
        error_code(resp),
        Some(code),
        "{context}: expected code {code}, got: {resp}"
    );
    assert_eq!(
        error_reason(resp),
        Some(reason),
        "{context}: expected data.reason {reason:?}, got: {resp}"
    );
}

// ============================================================================
// Class 1 — POLICY: a forbidden target stays `-32602` and is labelled.
// ============================================================================

/// A loopback IP LITERAL is a policy refusal: `-32602` (unchanged) plus the
/// `forbidden_ip_literal` slug.
///
/// The code is load-bearing and was deliberately NOT moved to `-32603`:
/// `mcp_ssrf_knob_matrix_test` pins it at exactly `-32602` and the SSRF knob
/// docs advertise that shape. The slug is what now tells a policy refusal apart
/// from a caller-input rejection, which shares the same code.
#[tokio::test]
async fn loopback_literal_is_policy_refusal_with_reason_slug() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "http://127.0.0.1:9/" }),
    )
    .await;

    assert_class(
        &resp,
        JSONRPC_INVALID_PARAMS,
        "forbidden_ip_literal",
        "loopback literal must be a policy refusal",
    );
}

/// A hostname that resolves into a forbidden range is ALSO policy, and is
/// separated from the literal case by its slug alone (`forbidden_ip_resolved`
/// vs `forbidden_ip_literal`) — both carry `-32602`.
///
/// Reached without network access: `localhost` is mapped by the local resolver
/// straight from the hosts database, never over the wire. The preconditions are
/// asserted explicitly first, so an environment that cannot supply a loopback
/// mapping fails LOUDLY with a precise message instead of being papered over
/// with a weakened assertion.
#[tokio::test]
async fn hostname_resolving_to_loopback_is_policy_refusal_with_reason_slug() {
    use webfang_core::domain::ssrf_guard::is_forbidden_ip;

    // Precondition: `localhost` must resolve locally, and every answer must be
    // a forbidden address — otherwise this fixture would exercise a DNS branch
    // (infrastructure) rather than the policy branch it claims to cover.
    let resolved: Vec<_> = tokio::net::lookup_host("localhost:80")
        .await
        .expect("precondition: `localhost` must resolve from the local hosts database")
        .map(|a| a.ip())
        .collect();
    assert!(
        !resolved.is_empty(),
        "precondition: `localhost` must yield at least one address"
    );
    assert!(
        resolved.iter().all(is_forbidden_ip),
        "precondition: every `localhost` answer must be a forbidden address, got: {resolved:?}"
    );

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "http://localhost:9/" }),
    )
    .await;

    assert_class(
        &resp,
        JSONRPC_INVALID_PARAMS,
        "forbidden_ip_resolved",
        "hostname resolving into a forbidden range must be a policy refusal",
    );
}

// ============================================================================
// Class 2 — INFRASTRUCTURE: a DNS failure is the server's fault (-32603).
// ============================================================================

/// An unresolvable host is an INFRASTRUCTURE failure, not a policy refusal and
/// not bad caller input: the resolver itself failed, so the code moves from
/// `-32602` to `-32603` (issue #1613 EC-03).
///
/// The slug is asserted by set membership rather than as one exact string,
/// because the two DNS branches (`dns_resolution_failed` for a resolver `Err`,
/// `dns_no_addresses` for an empty answer set) are the same class and the same
/// code — and which one a `.invalid` name lands on depends on the resolver in
/// front of the test (systemd-resolved returns NXDOMAIN as an error; a stub
/// may return an empty RRset). What the test pins is the class, not the
/// resolver's mood.
#[tokio::test]
async fn unresolvable_host_is_infrastructure_failure_with_reason_slug() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": UNRESOLVABLE_URL }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INTERNAL_ERROR),
        "a DNS failure must be reported as -32603 (server fault), not -32602 \
         (caller/policy): {resp}"
    );
    let reason = error_reason(&resp)
        .unwrap_or_else(|| panic!("a DNS failure must carry a data.reason slug: {resp}"));
    assert!(
        DNS_REASONS.contains(&reason),
        "a DNS failure must carry one of {DNS_REASONS:?}, got {reason:?}: {resp}"
    );
}

// ============================================================================
// Class 3 — CALLER INPUT: unaffected, and still field-tagged.
// ============================================================================

/// A params-validation rejection is untouched by EC-03 and keeps its field tag
/// as `data`.
///
/// This is the regression guard for the OTHER `-32602` shape in the protocol:
/// `validation::invalid_params` attaches `Some(Value::String(field))` — a bare
/// string, not the `{"reason": …}` object the SSRF pre-check now uses. Both are
/// `-32602`, so this test exists to prove the two shapes stay distinguishable
/// and that broadening the SSRF payload did not normalise the params one.
#[tokio::test]
async fn params_validation_rejection_keeps_field_tag_data() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "crawl_site",
        json!({ "url": "https://example.com", "max_depth": 11 }),
    )
    .await;

    assert_eq!(
        error_code(&resp),
        Some(JSONRPC_INVALID_PARAMS),
        "max_depth above the cap must stay a -32602 caller-input rejection: {resp}"
    );
    assert_eq!(
        resp.get("error").and_then(|e| e.get("data")),
        Some(&json!("max_depth")),
        "caller-input `data` must remain the bare field tag, unchanged by EC-03: {resp}"
    );
}
