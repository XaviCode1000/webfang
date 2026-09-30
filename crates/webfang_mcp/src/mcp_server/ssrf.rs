//! SSRF Protection — DNS resolution + IP validation
//!
//! Prevents Server-Side Request Forgery by resolving the host to an IP
//! and validating it against a deny list of forbidden ranges.

use rmcp::ErrorData as McpError;
use serde_json::Value;
use std::net::IpAddr;
use tokio::net::lookup_host;
// Pure IP deny-list logic lives in `webfang_core::domain::ssrf_guard` so the
// synchronous `wreq` redirect policy can reuse it; MCP depends on core, never
// the other way around (#703).
use webfang_core::domain::ssrf_guard::{
    disables_ssrf, is_forbidden_ip, parse_ip_literal, WEBFANG_MCP_DISABLE_SSRF_ENV,
};

/// Check if SSRF protection is enabled (based on environment variable).
///
/// SSRF is enabled by default. Set `WEBFANG_MCP_DISABLE_SSRF=1` to disable
/// for testing environments (e.g., when wiremock runs on 127.0.0.1).
///
/// Scope of that switch (#1294 P6-3): it disarms **this function only** — the
/// MCP-level DNS pre-check. The three per-layer disarmers that live in core
/// (`WEBFANG_DISABLE_SSRF_ENTRY_GUARD`, `..._REDIRECT_GUARD`, `..._RESOLVER`) are
/// independent, which is why an MCP test harness needs two variables where the CLI
/// harness needs one. The full matrix, and the reason the two stacks look different
/// while sharing one deny list, is in `docs/ssrf-layers.md`.
fn is_ssrf_enabled() -> bool {
    let raw = std::env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).ok();
    if raw
        .as_deref()
        .is_some_and(|value| !disables_ssrf(Some(value)))
    {
        warn_invalid_disable_value(raw.as_deref());
    }

    !disables_ssrf(raw.as_deref())
}

fn warn_invalid_disable_value(value: Option<&str>) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            variable = WEBFANG_MCP_DISABLE_SSRF_ENV,
            value = ?value,
            "WEBFANG_MCP_DISABLE_SSRF has a present invalid value; only the exact value \
             \"1\" disables the MCP SSRF entry pre-check"
        );
    });
}

/// Validate that a URL doesn't point to internal/private/forbidden IPs.
///
/// Resolves the host via DNS and checks every returned address against a
/// deny list that covers loopback, private, link-local, CGNAT (100.64.0.0/10),
/// IPv6 unique-local and unspecified ranges, plus IPv4-mapped/compatible
/// IPv6 addresses (re-validated against the IPv4 deny list).
///
/// Layered contract: this entry-level check is fast-fail typed UX; it is NOT
/// the enforcement point. Every scrape client obtains its protection from the
/// `webfang_core::domain::ssrf_guard::SsrfGuard` port, whose
/// `secure_client` installs the literal-IP redirect guard and the
/// `webfang_core::infrastructure::ssrf::ValidatingResolver` DNS guard, so
/// every DNS answer is re-validated at connect time —
/// covering hostname redirect hops and DNS-rebinding TOCTOU that this
/// entry check cannot see.
///
/// # Errors
///
/// Three failure CLASSES, deliberately machine-distinguishable via the JSON-RPC
/// `code` and the `data.reason` slug (EC-03, issue #1613). Before this split
/// every branch returned `-32602` with `data: None`, so a transient DNS outage
/// was indistinguishable from a caller aiming at internal infrastructure and
/// could only be told apart by parsing Spanish message text.
///
/// | class | branch | code | `data.reason` |
/// | :--- | :--- | :--- | :--- |
/// | caller input | URL has no host | `-32602` invalid_params | `no_host` |
/// | policy | IP literal in a forbidden range | `-32602` invalid_params | `forbidden_ip_literal` |
/// | infrastructure | resolver returned an error | `-32603` internal_error | `dns_resolution_failed` |
/// | infrastructure | resolver returned an empty answer set | `-32603` internal_error | `dns_no_addresses` |
/// | policy | a resolved address is forbidden | `-32602` invalid_params | `forbidden_ip_resolved` |
///
/// **The reason slugs are a stable contract.** `forbidden_ip_literal` and
/// `forbidden_ip_resolved` in particular: policy and caller input both surface
/// as `-32602` on purpose (the knob-matrix suite pins the loopback refusal at
/// exactly that code, and the SSRF knob docs advertise that shape), so the slug
/// is the ONLY thing separating them — it is load-bearing, not decoration.
/// Downstream tooling branches on it; do not rename or drop a slug without a
/// matching decision-table change.
///
/// `-32603` on the DNS branches is semantic, not cosmetic: a resolver failure
/// is the server's infrastructure failing, not the caller's argument being
/// wrong, which is the same class of fault this crate already reports through
/// `McpError::internal_error` (`handlers/scraping.rs`, `macros.rs`).
pub async fn validate_url_no_ssrf(url: &url::Url) -> Result<(), McpError> {
    // Skip validation if SSRF is disabled (e.g., in tests)
    if !is_ssrf_enabled() {
        // #1294 P6-3: this variable lifts exactly one layer, and the previous single
        // debug line read as "protection off". State what is still armed instead —
        // once per process, because this runs on every tool call.
        static WARNED: std::sync::Once = std::sync::Once::new();
        WARNED.call_once(|| {
            tracing::warn!(
                url = %url,
                "SSRF entry pre-check disabled by WEBFANG_MCP_DISABLE_SSRF: hostname \
                 targets and redirect hops are still validated at connect time, but IP \
                 literals are not checked on this path anymore (the core literal guard \
                 covers the CLI and the asset fetch router, and wreq never consults the \
                 validating resolver for a literal host). See docs/ssrf-layers.md"
            );
        });
        return Ok(());
    }

    tracing::debug!(url = %url, "SSRF protection enabled, validating");

    // CLASS: caller input — the URL itself carries no host, so no policy and no
    // infrastructure is involved. Unchanged `-32602`; the slug keeps it apart
    // from the policy refusals below.
    let host = url.host_str().ok_or_else(|| {
        McpError::invalid_params("URL sin host".to_string(), Some(reason(NO_HOST)))
    })?;

    // Shared literal-IP entry fast path (F-06 + F-32, #1217): the same
    // choke-point check the CLI request path enforces, applied here before any
    // DNS round-trip. Message and code are unchanged from the DNS path below
    // so existing `-32602` / `SSRF detectado` probes keep passing; hostnames
    // and public literals fall through to the resolving validator.
    if let Some(ip) = parse_ip_literal(host) {
        // CLASS: policy — the caller aimed at internal infrastructure. Stays
        // `-32602` (pinned by `mcp_ssrf_knob_matrix_test`); the slug is what
        // distinguishes it from `no_host` and from the resolved case below.
        if is_forbidden_ip(&ip) {
            return Err(McpError::invalid_params(
                format!(
                    "SSRF detectado: IP {ip} prohibida (acceso a red interna/cloud metadata bloqueado)"
                ),
                Some(reason(FORBIDDEN_IP_LITERAL)),
            ));
        }
    }

    let port = url
        .port()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });

    let addrs: Vec<_> = lookup_host(format!("{host}:{port}"))
        .await
        // CLASS: infrastructure — the resolver itself failed (no such record,
        // SERVFAIL, timeout). The caller's argument is fine, so this is the
        // server's fault: `-32603`.
        .map_err(|e| {
            McpError::internal_error(
                format!("error de resolución DNS para '{host}': {e}"),
                Some(reason(DNS_RESOLUTION_FAILED)),
            )
        })?
        .collect();

    if addrs.is_empty() {
        // CLASS: infrastructure — the resolver answered but returned no
        // address. Same code as the resolver-err branch above; the slugs differ
        // so an operator can tell "the query failed" from "the name has no
        // addresses", but both are server-side, so both are `-32603`.
        return Err(McpError::internal_error(
            format!("no se pudo resolver la IP para '{host}'"),
            Some(reason(DNS_NO_ADDRESSES)),
        ));
    }

    for addr in &addrs {
        let ip: IpAddr = addr.ip();
        // CLASS: policy — the hostname resolved into a forbidden range, so the
        // target is internal infrastructure. Stays `-32602` like the literal
        // branch; `forbidden_ip_resolved` is what separates the two.
        if is_forbidden_ip(&ip) {
            return Err(McpError::invalid_params(
                format!(
                    "SSRF detectado: IP {ip} prohibida (acceso a red interna/cloud metadata bloqueado)"
                ),
                Some(reason(FORBIDDEN_IP_RESOLVED)),
            ));
        }
    }

    Ok(())
}

// ============================================================================
// Error-reason slugs — the machine-readable half of the decision table
// documented on `validate_url_no_ssrf`. These string values are a contract:
// downstream tooling branches on them, so they live in named constants rather
// than as literals scattered across the five call sites (one typo must not be
// able to split a slug in two).
// ============================================================================

/// CLASS caller input — the URL has no host component.
const NO_HOST: &str = "no_host";
/// CLASS policy — the URL host is an IP literal inside a forbidden range.
const FORBIDDEN_IP_LITERAL: &str = "forbidden_ip_literal";
/// CLASS infrastructure — the DNS resolver returned an error.
const DNS_RESOLUTION_FAILED: &str = "dns_resolution_failed";
/// CLASS infrastructure — the DNS resolver returned an empty answer set.
const DNS_NO_ADDRESSES: &str = "dns_no_addresses";
/// CLASS policy — a resolved address is inside a forbidden range.
const FORBIDDEN_IP_RESOLVED: &str = "forbidden_ip_resolved";

/// Wrap a reason slug into the `data` payload every branch of
/// `validate_url_no_ssrf` attaches, so a caller can classify the failure
/// without parsing the (Spanish, human-facing) message text.
fn reason(slug: &str) -> Value {
    serde_json::json!({ "reason": slug })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // -------------------------------------------------------------------
    // EC-03 reason-slug contract (#1613). The integration proof lives in
    // `tests/mcp_ssrf_error_class_test.rs` (over a live server); these pin the
    // two properties that test cannot see from the outside — the `data` shape
    // and the fact that the five slugs stay DISTINCT, which is the property
    // that makes them load-bearing rather than decorative.
    // -------------------------------------------------------------------

    #[test]
    fn reason_payload_is_a_machine_readable_reason_object() {
        assert_eq!(
            reason(FORBIDDEN_IP_LITERAL),
            serde_json::json!({ "reason": "forbidden_ip_literal" })
        );
    }

    #[test]
    fn reason_slugs_are_distinct() {
        let slugs = [
            NO_HOST,
            FORBIDDEN_IP_LITERAL,
            DNS_RESOLUTION_FAILED,
            DNS_NO_ADDRESSES,
            FORBIDDEN_IP_RESOLVED,
        ];
        for (i, a) in slugs.iter().enumerate() {
            for b in &slugs[i + 1..] {
                assert_ne!(a, b, "two failure classes must not share a reason slug");
            }
        }
    }

    #[test]
    fn loopback_v4_is_forbidden() {
        assert!(is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            127, 0, 0, 1
        ))));
    }

    #[test]
    fn private_v4_is_forbidden() {
        assert!(is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            10, 0, 0, 1
        ))));
        assert!(is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            192, 168, 1, 1
        ))));
        assert!(is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            172, 16, 0, 1
        ))));
    }

    #[test]
    fn link_local_v4_is_forbidden() {
        assert!(is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            169, 254, 1, 1
        ))));
    }

    #[test]
    fn cgnat_v4_is_forbidden() {
        assert!(is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            100, 64, 0, 1
        ))));
        assert!(is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            100, 127, 255, 255
        ))));
        assert!(!is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            100, 63, 255, 255
        ))));
        assert!(!is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            100, 128, 0, 1
        ))));
    }

    #[test]
    fn public_v4_is_allowed() {
        assert!(!is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            8, 8, 8, 8
        ))));
        assert!(!is_forbidden_ip(&IpAddr::V4(std::net::Ipv4Addr::new(
            1, 1, 1, 1
        ))));
    }

    #[test]
    fn loopback_v6_is_forbidden() {
        assert!(is_forbidden_ip(&IpAddr::V6(std::net::Ipv6Addr::new(
            0, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn unique_local_v6_is_forbidden() {
        assert!(is_forbidden_ip(&IpAddr::V6(std::net::Ipv6Addr::new(
            0xfc00, 0, 0, 0, 0, 0, 0, 1
        ))));
        assert!(is_forbidden_ip(&IpAddr::V6(std::net::Ipv6Addr::new(
            0xfd00, 0, 0, 0, 0, 0, 0, 1
        ))));
    }

    #[test]
    fn public_v6_is_allowed() {
        assert!(!is_forbidden_ip(&IpAddr::V6(std::net::Ipv6Addr::new(
            0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111
        ))));
    }

    // --- IPv4-mapped / IPv4-compatible bypass tests (#703) ---

    #[test]
    fn ipv4_mapped_loopback_is_forbidden() {
        let ip: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_forbidden_ip(&ip));
    }

    #[test]
    fn ipv4_mapped_link_local_is_forbidden() {
        let ip: IpAddr = "::ffff:169.254.169.254".parse().unwrap();
        assert!(is_forbidden_ip(&ip));
    }

    #[test]
    fn ipv4_mapped_cgnat_is_forbidden() {
        let ip: IpAddr = "::ffff:100.64.0.1".parse().unwrap();
        assert!(is_forbidden_ip(&ip));
    }

    #[test]
    fn ipv4_mapped_public_is_allowed() {
        let ip: IpAddr = "::ffff:8.8.8.8".parse().unwrap();
        assert!(!is_forbidden_ip(&ip));
    }

    #[test]
    fn unspecified_v6_is_forbidden() {
        let ip: IpAddr = "::".parse().unwrap();
        assert!(is_forbidden_ip(&ip));
    }

    #[test]
    fn ipv4_compatible_loopback_is_forbidden() {
        // Deprecated IPv4-compatible form ::a.b.c.d must also be re-validated.
        let ip: IpAddr = "::127.0.0.1".parse().unwrap();
        assert!(is_forbidden_ip(&ip));
    }

    proptest! {
        #[test]
        fn mapped_equivalence_v4_v6(a: u8, b: u8, c: u8, d: u8) {
            let v4 = std::net::Ipv4Addr::new(a, b, c, d);
            let v6_mapped = v4.to_ipv6_mapped();
            prop_assert_eq!(
                is_forbidden_ip(&IpAddr::V4(v4)),
                is_forbidden_ip(&IpAddr::V6(v6_mapped))
            );
        }
    }
}
