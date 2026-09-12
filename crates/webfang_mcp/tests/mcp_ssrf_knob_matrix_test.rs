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
//! **The layering does.** Both stacks decide with the same predicate, but MCP has one
//! each layer has its own test-only kill-switch:
//!
//! | Layer | Sees | CLI | MCP |
//! | :--- | :--- | :--- | :--- |
//! | MCP DNS pre-check (`mcp_server/ssrf.rs:41-105`) | literals **and** hostnames (own `lookup_host`) | — | `WEBFANG_MCP_DISABLE_SSRF` |
//! | core literal entry guard (`reject_forbidden_literal_url`, `ssrf_guard.rs:373`) | literals only | `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` | on the MCP scrape path since #1301 (scraper_service pre-check) |
//! | connect-time resolver (`infrastructure/ssrf.rs`) | hostnames only — wreq short-circuits IP literals before any custom resolver (`ssrf.rs:105-108`) | same env family | same env family |
//!
//! Consequence, and the thing an operator must not have to guess (#1294 P6-3): the
//! core literal entry guard is wired in `cli/scrape_flow.rs:464`,
//! `infrastructure/downloader/fetch_router.rs:188`, and — since #1301 — the MCP
//! scrape path (scraper_service pre-check), which goes through
//! `application::scraper_service::scrape_with_config`. Since wreq consults no
//! resolver for an IP-literal host ("wreq short-circuits IP-literal hosts",
//! `infrastructure/ssrf.rs:105-108`), lifting only `WEBFANG_MCP_DISABLE_SSRF` no
//! longer leaves such a target unchecked on this path: both knobs compose, and that
//! is the scope this suite pins. Pointing the server at an internal target
//! deliberately still takes both switches — and it has the same scope as the CLI's
//! `WEBFANG_DISABLE_SSRF_ENTRY_GUARD`, which `cli_harness.rs:142-150` disarms and then
//! drives `127.0.0.1` mocks through. What was wrong is that nothing said so: the binary
//! logged "SSRF protection disabled (test mode)" and the pre-check logged at `debug`.
//! Both now name the layers that stay armed, and `docs/ssrf-layers.md` holds the matrix.
//!
//! Every test here is therefore a characterization: it pins the intended scope in both
//! directions, so changing any layer has to pass here first.
//!
//! Run with: `cargo nextest run -p webfang_mcp --features mcp --test mcp_ssrf_knob_matrix_test`

#![cfg(feature = "mcp")]

mod common;
use common::*;

use serde_json::{json, Value};
use std::net::IpAddr;
use webfang_mcp::mcp_server::ssrf::validate_url_no_ssrf;
use webfang_test_utils::EnvGuard;

/// MCP's own entry-layer kill-switch (`mcp_server/ssrf.rs`), referenced
/// through the canonical SSOT constant so a rename cannot silently
/// desynchronize this suite.
use webfang_core::domain::ssrf_guard::{
    is_forbidden_ip, WEBFANG_MCP_DISABLE_SSRF_ENV as MCP_SSRF_ENV,
};

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

/// P6-3 parity claim, stated executably: MCP's entry switch has the SAME scope
/// for IP literals as the CLI's.
///
/// With both SSRF knobs set, nothing refuses an IP literal on the MCP scrape
/// path: the MCP pre-check is lifted by `WEBFANG_MCP_DISABLE_SSRF` and the core
/// literal guard — consulted on this path since #1301 via the scraper_service
/// pre-check — by `WEBFANG_DISABLE_SSRF_ENTRY_GUARD`. The request then reaches
/// the socket and fails as a connection error with no SSRF wording. The CLI-side
/// twin of that exact fact is
/// `domain::ssrf_guard`'s `entry_guard_hatch_requires_exact_value_one` (`"1"`
/// disarms, which is how `tests/common/cli_harness.rs` drives `127.0.0.1` mocks).
/// Characterization, not approval: hostname targets and redirect hops stay
/// validated, and `docs/ssrf-layers.md` maps knob to layer.
#[tokio::test]
async fn mcp_entry_env_has_the_same_literal_scope_as_the_cli_entry_env() {
    // Built with every guard armed; the starter removes the disable flag itself.
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = wreq::Client::new();
    let session_id = init_session(&client, &base_url).await;

    // Both knobs: since #1301 the MCP scrape path consults the core literal
    // entry guard too (scraper_service pre-check), so the MCP knob alone no
    // longer leaves an IP literal unchecked here.
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
        "with both SSRF knobs lifted no SSRF refusal may appear: {resp}"
    );
    assert!(
        is_failure(&resp),
        "a closed discard port must still fail the tool call, not silently succeed: {resp}"
    );
}

/// P6-3 verdict, part 3: the reverse direction, the core kill-switch does not
/// disarm MCP's layer.
///
/// With only `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` set, this crate's pre-check stays
/// armed and still refuses the loopback literal with the protocol-level `-32602`.
/// That single pair of results is the whole P6-3 answer: the stacks share one deny
/// list and one verdict, and what differs is how many layers each one has to disarm
/// — which is why the MCP harnesses set two variables where `cli_harness.rs` sets
/// one. Documented, not relaxed.
#[tokio::test]
async fn core_entry_guard_env_does_not_lift_the_mcp_precheck() {
    let (base_url, _handle) = start_test_server_ssrf_enabled().await;
    let client = wreq::Client::new();
    let session_id = init_session(&client, &base_url).await;

    let mut guard = EnvGuard::clean(&[MCP_SSRF_ENV]);
    guard.set(CORE_ENTRY_GUARD_ENV, "1");

    let url = "http://127.0.0.1:9/";
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": url }),
    )
    .await;

    assert_ssrf_refusal(&resp, url, "only the core entry layer was disarmed");
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
