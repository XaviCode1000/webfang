//! SSRF Protection — DNS resolution + IP validation
//!
//! Prevents Server-Side Request Forgery by resolving the host to an IP
//! and validating it against a deny list of forbidden ranges.

use rmcp::ErrorData as McpError;
use std::net::IpAddr;
use tokio::net::lookup_host;
// Pure IP deny-list logic lives in `webfang_core::domain::ssrf_guard` so the
// synchronous `wreq` redirect policy can reuse it; MCP depends on core, never
// the other way around (#703).
use webfang_core::domain::ssrf_guard::is_forbidden_ip;

/// Check if SSRF protection is enabled (based on environment variable).
///
/// SSRF is enabled by default. Set `WEBFANG_MCP_DISABLE_SSRF=1` to disable
/// for testing environments (e.g., when wiremock runs on 127.0.0.1).
fn is_ssrf_enabled() -> bool {
    std::env::var("WEBFANG_MCP_DISABLE_SSRF").is_err()
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
/// Returns `McpError::invalid_params` if the URL has no host, DNS resolution
/// fails, or any resolved IP falls within a forbidden range.
pub async fn validate_url_no_ssrf(url: &url::Url) -> Result<(), McpError> {
    // Skip validation if SSRF is disabled (e.g., in tests)
    if !is_ssrf_enabled() {
        tracing::debug!(url = %url, "SSRF protection skipped (disabled via env var)");
        return Ok(());
    }

    tracing::debug!(url = %url, "SSRF protection enabled, validating");

    let host = url
        .host_str()
        .ok_or_else(|| McpError::invalid_params("URL sin host".to_string(), None))?;

    let port = url
        .port()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });

    let addrs: Vec<_> = lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|e| {
            McpError::invalid_params(format!("error de resolución DNS para '{host}': {e}"), None)
        })?
        .collect();

    if addrs.is_empty() {
        return Err(McpError::invalid_params(
            format!("no se pudo resolver la IP para '{host}'"),
            None,
        ));
    }

    for addr in &addrs {
        let ip: IpAddr = addr.ip();
        if is_forbidden_ip(&ip) {
            return Err(McpError::invalid_params(
                format!(
                    "SSRF detectado: IP {ip} prohibida (acceso a red interna/cloud metadata bloqueado)"
                ),
                None,
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

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
