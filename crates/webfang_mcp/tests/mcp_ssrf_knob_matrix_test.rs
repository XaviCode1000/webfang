//! MCP SSRF layer/knob matrix (issue #1294, P6-3).
//!
//! The reported symptom is a divergence: "the CLI sitemap works on loopback
//! fixtures, MCP blocks". This suite separates the two things that were conflated
//! and gives the verdict the issue asks for.
//!
//! **The policy does not diverge.** Both stacks decide with the *same* predicate,
//! [`is_forbidden_ip`] (`domain/ssrf_guard.rs:96-107`, loopback included), which
//! MCP imports directly (`mcp_server/ssrf.rs:12`). [`policy_parity_…`] proves it
//! on one address table.
//!
//! **The layering does.** The chain has one more entry layer on each side, and
//! each layer has its own test-only kill-switch:
//!
//! | Layer | Sees | CLI | MCP |
//! | :--- | :--- | :--- | :--- |
//! | MCP DNS pre-check (`mcp_server/ssrf.rs:41-105`) | literals **and** hostnames (own `lookup_host`) | — | `WEBFANG_MCP_DISABLE_SSRF` |
//! | core literal entry guard (`reject_forbidden_literal_url`, `ssrf_guard.rs:373`) | literals only | `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` | **absent from this path today** |
//! | connect-time resolver (`infrastructure/ssrf.rs`) | hostnames only — wreq short-circuits IP literals before any custom resolver (`ssrf.rs:105-108`) | same env family | same env family |
//!
//! Consequence, and the part that is an actual defect rather than policy: the
//! core literal guard is wired in `cli/scrape_flow.rs:464` and
//! `infrastructure/downloader/fetch_router.rs:188`, but the MCP scrape path goes
//! through `application::scraper_service::scrape_with_config`, which has no entry
//! guard. So for MCP, `WEBFANG_MCP_DISABLE_SSRF` is not "one layer off" — with the
//! resolver structurally blind to IP literals, it is the **only** thing standing
//! between a loopback target and an opened socket, while `bin/mcp_server_http.rs`
//! logs it as "SSRF protection disabled (test mode)".
//!
//! That is what [`mcp_disable_env_lifts_only_its_own_precheck`] pins. It is the
//! suite's only **red** test against the current build; the others are
//! characterizations that document the intended policy so P6-3 closes with
//! evidence rather than with a relaxation.
//!
//! Run with: `cargo nextest run -p webfang_mcp --features mcp --test mcp_ssrf_knob_matrix_test`

#![cfg(feature = "mcp")]

mod common;
use common::*;

use serde_json::{json, Value};
use std::net::IpAddr;
use webfang_core::domain::ssrf_guard::is_forbidden_ip;
use webfang_mcp::mcp_server::ssrf::validate_url_no_ssrf;
use webfang_test_utils::EnvGuard;

/// MCP's own entry-layer kill-switch (`mcp_server/ssrf.rs:19`).
const MCP_SSRF_ENV: &str = "WEBFANG_MCP_DISABLE_SSRF";

/// Core's literal-entry-guard kill-switch, referenced through the SSOT constant
/// so a rename cannot silently desynchronize this suite.
const CORE_ENTRY_GUARD_ENV: &str = webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;

/// JSON-RPC "Invalid params" — the channel both SSRF layers surface through.
const JSONRPC_INVALID_PARAMS: i64 = -32602;

/// Shared Spanish marker of an SSRF rejection (both layers say `SSRF detectado`).
const SSRF_MARKER: &str = "SSRF detectado";

/// Forbidden literals the deny list must cover, plus a public control.
const FORBIDDEN_LITERALS: &[&str] = &[
    "127.0.0.1",          // loopback — the fixture address in the report
    "10.0.0.1",           // private
    "172.16.0.1",         // private
    "192.168.1.1",        // private
    "169.254.169.254",    // link-local / cloud metadata
    "100.64.0.1",         // CGNAT
    "0.0.0.0",            // unspecified (routes to loopback on Linux)
    "[::1]",              // IPv6 loopback
    "[::ffff:127.0.0.1]", // IPv4-mapped IPv6 loopback
];

/// Parse and reject one literal through MCP's entry layer.
async fn mcp_rejects(url_str: &str) -> bool {
    let url = url::Url::parse(url_str).expect("test fixture must be a valid URL");
    validate_url_no_ssrf(&url).await.is_err()
}

// ============================================================================
// Policy parity: the same predicate, not two different ones
// ============================================================================

/// P6-3 policy verdict, part 1: MCP and the CLI share ONE deny list.
///
/// For every forbidden literal, the core predicate the CLI entry guard uses and
/// MCP's entry check must agree; the public control must be accepted by both. If
/// this ever disagrees, the divergence is real policy and belongs in core, not in
/// either transport.
#[tokio::test]
async fn policy_parity_forbidden_literals_rejected_by_both_predicates() {
    // The guard must be armed for this to mean anything.
    let _guard = EnvGuard::clean(&[MCP_SSRF_ENV, CORE_ENTRY_GUARD_ENV]);

    for literal in FORBIDDEN_LITERALS {
        let url_str = format!("http://{literal}:9/");
        let ip: IpAddr = literal
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .unwrap_or_else(|e| panic!("fixture {literal} must be an IP literal: {e}"));

        assert!(
            is_forbidden_ip(&ip),
            "core deny list (used by the CLI entry guard) must forbid {literal}"
        );
        assert!(
            mcp_rejects(&url_str).await,
            "MCP's entry check must forbid the same {url_str}"
        );
    }

    let control = "http://8.8.8.8:9/";
    let control_ip: IpAddr = "8.8.8.8".parse().expect("public control is an IP");
    assert!(
        !is_forbidden_ip(&control_ip),
        "public control must not be forbidden, or the suite proves nothing"
    );
    assert!(
        !mcp_rejects(control).await,
        "MCP must not reject the public control {control}"
    );
}

// ============================================================================
// Layer independence: which knob lifts what
// ============================================================================

/// P6-3 verdict, part 2: `WEBFANG_MCP_DISABLE_SSRF` is an entry-layer switch,
/// never a global one. Loopback must still be refused by the shared core entry
/// guard while only that variable is set.
///
/// **RED against the current build.** The core literal guard is not on the MCP
/// scrape path, and wreq never consults the validating resolver for IP literals,
/// so today this probe opens (and fails) a socket instead of being refused at
/// entry. The fix keeps the intended policy and adds the shared guard to the MCP
/// path; the companion test below shows what legitimately does open the path.
#[tokio::test]
async fn mcp_disable_env_lifts_only_its_own_precheck() {
    // Built with every guard armed; the starter removes the disable flag itself.
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = wreq::Client::new();
    let session_id = init_session(&client, &base_url).await;

    // Set only MCP's knob; explicitly lift the core one out of any inherited env.
    let mut guard = EnvGuard::with(&[(MCP_SSRF_ENV, "1")]);
    guard.remove(CORE_ENTRY_GUARD_ENV);

    let url = "http://127.0.0.1:9/";
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": url }),
    )
    .await;

    assert_ssrf_refusal(&resp, url, "MCP's entry layer is disabled in this probe");
}

/// The honest companion: it takes BOTH knobs to put a loopback literal on the
/// wire. That is why the MCP harnesses set two variables where the CLI harness
/// sets one (`tests/common/mod.rs` vs `cli_harness.rs:145-150`), and it is the
/// asymmetry this issue must document rather than "fix" by relaxing a guard.
///
/// Characterization: passes today and after the fix.
#[tokio::test]
async fn both_entry_knobs_are_what_reach_the_socket() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = wreq::Client::new();
    let session_id = init_session(&client, &base_url).await;

    let _guard = EnvGuard::with(&[(MCP_SSRF_ENV, "1"), (CORE_ENTRY_GUARD_ENV, "1")]);

    let url = "http://127.0.0.1:9/";
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": url }),
    )
    .await;

    assert!(
        !carries_ssrf_marker(&resp),
        "with both entry layers lifted the request must go to the socket, so no SSRF \
         refusal may be reported; the failure has to be a connection error. Got: {resp}"
    );
    assert!(
        is_failure(&resp),
        "a closed discard port must still fail the tool call, not silently succeed: {resp}"
    );
}

// ============================================================================
// Assertions
// ============================================================================

/// Whether a JSON-RPC envelope carries the Spanish SSRF marker anywhere in it.
fn carries_ssrf_marker(resp: &Value) -> bool {
    resp.to_string().contains(SSRF_MARKER)
}

/// Whether the envelope is a failure of either channel (protocol error or
/// `isError` tool result).
fn is_failure(resp: &Value) -> bool {
    if resp.get("error").is_some() {
        return true;
    }
    resp.get("result")
        .and_then(|r| r.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Assert the envelope is the protocol-level SSRF refusal: JSON-RPC `-32602`
/// carrying `SSRF detectado`.
///
/// Both entry layers surface through this channel, so the assertion holds no
/// matter which one caught the target — which is exactly the property under test:
/// an IP literal must never reach the socket while any entry guard is armed.
fn assert_ssrf_refusal(resp: &Value, url: &str, context: &str) {
    assert!(
        carries_ssrf_marker(resp),
        "scrape_url({url}) must be refused by an armed entry guard ({context}), got: {resp}"
    );
    let code = resp
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    assert_eq!(
        code, JSONRPC_INVALID_PARAMS,
        "scrape_url({url}) must stay a protocol-level -32602 refusal ({context}), got: {resp}"
    );
}
