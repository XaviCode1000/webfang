//! E2E suite: per-provider `NetworkPolicy` (B2 slice 2, ADR-0004 M2,
//! issue #1947) reaching an allowlisted LAN embedding endpoint.
//!
//! | Case | Pinned outcome |
//! |------|----------------|
//! | allowlisted LAN literal (`http://10.x.x.x:8000`) | adapter constructs; dial fails at CONNECT, never at the guard |
//! | LAN literal NOT declared | rejected pre-socket by the entry gate, zero outbound |
//! | CGNAT `100.64.0.0/10` without opt-in | rejected |
//! | hostname + CIDR + `allow_hosts` | resolution permitted |
//! | hostname + CIDR WITHOUT `allow_hosts` | rejected, error names `allow_hosts` |
//! | `169.254.169.254` even if allowlisted | denied (FIN-017 invariant 1) |
//!
//! Determinism (test-quality rule 6) — the permitted cases dial
//! `10.255.255.x:9` (RFC1918, discard port): when the policy PERMITS, the
//! failure is a CONNECT-phase error (refused, unreachable, or the 2 s connect
//! timeout below); when it DENIES, it is the SSRF `ForbiddenResolutionError`.
//! No assertion here depends on how the machine routes 10.x.x.x.
//!
//! **What the permitted rows actually prove — and what they do not.** They
//! assert the ABSENCE of a guard rejection across the whole rendered error
//! chain, and that is deliberately the assertion made: it is the only one that
//! holds identically on a host that refuses, one that drops, and one behind a
//! proxy. A positive match on a CONNECT-phase wording would be a machine- and
//! platform-dependent test. The cost of that choice is stated plainly: these
//! rows would not catch a regression that replaced the guard with some other
//! pre-socket failure that also omits the guard's wording. The strong direction
//! (a denial IS produced) is pinned by rows 2-6; these rows pin the weak one
//! (no denial is produced) plus the construction succeeding.
//!
//! Hostname rows use `/etc/hosts`-free determinism: they run against the
//! public API verdict and against `ValidatingResolver::fail_closed_scan` with
//! synthetic answer sets (in-crate unit tests in `infrastructure::ssrf`), so no
//! DNS server is ever consulted.

use std::net::IpAddr;
use std::time::Duration;

use webfang_core::domain::network_policy::{CidrBlock, NetworkPolicy, NetworkPolicyMode};
use webfang_core::domain::providers::{Capability, ProviderConfig, ProviderKind};
use webfang_core::domain::ssrf_guard::{
    is_forbidden_literal_host, is_forbidden_literal_host_with_policy, is_forbidden_resolved_address,
};
use webfang_core::infrastructure::llm::provider::ProviderInitError;
use webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter;

/// Discard port on an RFC1918 address inside `10.255.255.0/24`: nothing
/// listens there, so a permitted dial runs out the connect path while a denied
/// dial never opens a socket.
const LAN_DISCARD: &str = "http://10.255.255.254:9/v1";

fn policy(cidrs: &[&str], hosts: &[&str]) -> NetworkPolicy {
    NetworkPolicy {
        mode: NetworkPolicyMode::Allowlist,
        allow_cidrs: cidrs
            .iter()
            .map(|c| CidrBlock::parse(c).expect("test CIDR parses"))
            .collect(),
        allow_hosts: hosts.iter().map(|h| h.to_ascii_lowercase()).collect(),
    }
}

fn provider(base_url: &str, network_policy: NetworkPolicy) -> ProviderConfig {
    ProviderConfig {
        id: "vllm-lan".to_string(),
        display_name: "vLLM LAN".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: url::Url::parse(base_url).expect("test base_url parses"),
        auth: webfang_core::domain::auth_source::AuthSource::None,
        capabilities: vec![Capability::Embedding],
        model: Some("granite-embedding".to_string()),
        embedding_dim: None,
        allow_loopback: false,
        network_policy,
    }
}

/// A client built through the same production choke point the adapter uses
/// (`SsrfGuard::secure_client_with_policy`), with a short connect timeout so a
/// permitted-but-unreachable dial cannot stall the suite.
fn policy_client(network_policy: &NetworkPolicy) -> wreq::Client {
    webfang_core::domain::ssrf_guard::ssrf_guard()
        .secure_client_with_policy(
            wreq::Client::builder().connect_timeout(Duration::from_secs(2)),
            false,
            network_policy,
        )
        .build()
        .expect("guarded test client builds")
}

/// Render a transport failure together with its whole `source` chain.
///
/// The guard's verdict does NOT live in the top-level `Display` — a denial
/// renders as `error sending request for uri (...)` -> `client error
/// (Connect)` -> `dns error` -> the resolver's message. Asserting on
/// `to_string()` alone would be vacuous: the denial would be invisible and the
/// row would pass no matter what. Walking the chain is what makes the
/// negative assertion below able to fail. Production mirrors this in
/// `remote_embedding::render_error_chain`.
fn rendered_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = err.to_string();
    let mut source = err.source();
    while let Some(inner) = source {
        rendered.push_str(": ");
        rendered.push_str(&inner.to_string());
        source = inner.source();
    }
    rendered
}

async fn dial_class(url: &str, network_policy: &NetworkPolicy) -> String {
    let err = policy_client(network_policy)
        .get(url)
        .send()
        .await
        .expect_err("nothing listens on the LAN discard target");
    rendered_chain(&err)
}

// ============================================================================
// Row 1 — allowlisted LAN literal: construction succeeds and the dial is not
// rejected by the guard anywhere in the error chain. Asymmetric on purpose: a
// literal never travels the hostname path, so `allow_cidrs` alone suffices.
// ============================================================================

#[tokio::test]
async fn row1_allowlisted_lan_literal_reaches_connect() {
    let net = policy(&["10.0.0.0/8"], &[]);
    RemoteEmbeddingAdapter::new(provider(LAN_DISCARD, net.clone()))
        .expect("allowlisted LAN literal must build: allow_cidrs alone suffice for literals");

    let rendered = dial_class(LAN_DISCARD, &net).await;
    assert!(
        !rendered.contains("forbidden") && !rendered.contains("SSRF"),
        "a permitted LAN literal must not be denied by the guard: {rendered}"
    );
}

// ============================================================================
// Row 2 — LAN literal NOT declared: rejected before any socket.
// ============================================================================

#[tokio::test]
async fn row2_undeclared_lan_literal_is_rejected_at_entry() {
    let err = match RemoteEmbeddingAdapter::new(provider(LAN_DISCARD, NetworkPolicy::default())) {
        Ok(_) => panic!("an undeclared LAN literal stays forbidden"),
        Err(err) => err,
    };

    let text = match &err {
        ProviderInitError::InvalidBaseUrl { source, .. } => source.to_string(),
        other => panic!("expected the typed entry-gate rejection, got: {other}"),
    };
    assert!(
        text.contains("allow_loopback") || text.contains("allow_cidrs"),
        "the rejection must name the config field that can open a LAN host: {text}"
    );
    assert!(
        is_forbidden_literal_host("10.255.255.254"),
        "the pure entry verdict must agree with the adapter"
    );
}

// ============================================================================
// Row 3 — CGNAT 100.64.0.0/10 needs the explicit per-provider opt-in (M2).
// ============================================================================

#[test]
fn row3_cgnat_needs_explicit_opt_in() {
    let cgnat = "100.64.0.1";
    assert!(
        is_forbidden_literal_host_with_policy(cgnat, false, &NetworkPolicy::default()),
        "CGNAT stays denied without the explicit opt-in"
    );
    assert!(
        is_forbidden_literal_host_with_policy(cgnat, false, &policy(&["10.0.0.0/8"], &[])),
        "an unrelated allowlist entry must not open CGNAT"
    );
    assert!(
        !is_forbidden_literal_host_with_policy(cgnat, false, &policy(&["100.64.0.0/10"], &[])),
        "declaring the CGNAT range IS the opt-in"
    );
}

// ============================================================================
// Row 4/5 — the `allow_hosts` AND decision on the hostname path.
// ============================================================================

#[test]
fn row4_hostname_with_cidr_and_allow_hosts_is_permitted() {
    let net = policy(&["10.0.0.0/8"], &["vllm.lan"]);
    let ip: IpAddr = "10.0.0.5".parse().expect("test literal");
    assert!(
        !is_forbidden_resolved_address(&ip, "vllm.lan", false, &net),
        "declared hostname over an allowlisted network is permitted"
    );
    // The declared name matches case- and root-dot-insensitively.
    for host in ["VLLM.LAN", "vllm.lan."] {
        assert!(
            !is_forbidden_resolved_address(&ip, host, false, &net),
            "{host} is the same declared name"
        );
    }
}

#[test]
fn row5_hostname_with_cidr_without_allow_hosts_is_rejected() {
    let net = policy(&["10.0.0.0/8"], &[]);
    let ip: IpAddr = "10.0.0.5".parse().expect("test literal");
    assert!(
        is_forbidden_resolved_address(&ip, "vllm.lan", false, &net),
        "AND: an allowlisted network alone is the precondition, not the permission"
    );
    // A different declared host does not authorize this one, and an empty
    // policy is exactly today's behavior.
    let other = policy(&["10.0.0.0/8"], &["otro.lan"]);
    assert!(is_forbidden_resolved_address(
        &ip, "vllm.lan", false, &other
    ));
    assert!(is_forbidden_resolved_address(
        &ip,
        "vllm.lan",
        false,
        &NetworkPolicy::default()
    ));
    // Public addresses never need a declaration.
    let public: IpAddr = "8.8.8.8".parse().expect("test literal");
    assert!(!is_forbidden_resolved_address(
        &public,
        "cualquier.nombre",
        false,
        &NetworkPolicy::default()
    ));
}

// ============================================================================
// Row 6 — FIN-017 invariant 1: 169.254.169.254 stays denied even when a
// hand-built policy (one that bypassed config validation) covers it.
// ============================================================================

#[test]
fn row6_cloud_metadata_stays_denied_even_when_allowlisted() {
    let net = policy(&["169.254.0.0/16"], &["metadata.lan"]);
    let ip: IpAddr = "169.254.169.254".parse().expect("test literal");
    assert!(is_forbidden_literal_host_with_policy(
        "169.254.169.254",
        false,
        &net
    ));
    assert!(is_forbidden_resolved_address(
        &ip,
        "metadata.lan",
        false,
        &net
    ));
    assert!(is_forbidden_resolved_address(
        &ip,
        "metadata.lan",
        false,
        &NetworkPolicy::default()
    ));
}

// ============================================================================
// Vacuity guard for row 1. Row 1 asserts the ABSENCE of a guard denial across
// the rendered chain; an absence assertion that can never fail proves nothing.
// This row is the positive counterpart: a dial that IS denied must surface the
// guard's wording inside that same chain.
//
// `localhost` is the deterministic subject — it resolves to loopback, which
// the default policy denies and only `allow_loopback` could open, so the
// denial comes from the validating resolver, not the entry gate (a hostname
// passes the entry gate by construction). No LAN, no external network.
// ============================================================================

#[tokio::test]
async fn row7_a_guard_denial_is_visible_in_the_rendered_chain() {
    let net = NetworkPolicy::default();
    let err = policy_client(&net)
        .get("http://localhost:9/v1")
        .send()
        .await
        .expect_err("the default policy denies loopback for a hostname dial");
    let rendered = rendered_chain(&err);
    assert!(
        rendered.to_lowercase().contains("forbidden"),
        "a denial must be visible in the chain, otherwise row 1's negative \
         assertion is vacuous: {rendered}"
    );
}
