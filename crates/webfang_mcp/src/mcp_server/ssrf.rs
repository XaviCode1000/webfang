//! SSRF Protection — DNS resolution + IP validation
//!
//! Prevents Server-Side Request Forgery by resolving the host to an IP
//! and validating it against a deny list of forbidden ranges.

use rmcp::ErrorData as McpError;
use std::net::IpAddr;
use tokio::net::lookup_host;

/// Validate that a URL doesn't point to internal/private/forbidden IPs.
///
/// Resolves the host via DNS and checks every returned address against a
/// deny list that covers loopback, private, link-local, CGNAT (100.64.0.0/10),
/// and IPv6 unique-local ranges.
///
/// # Errors
/// Returns `McpError::invalid_params` if the URL has no host, DNS resolution
/// fails, or any resolved IP falls within a forbidden range.
pub async fn validate_url_no_ssrf(url: &url::Url) -> Result<(), McpError> {
    let host = url
        .host_str()
        .ok_or_else(|| McpError::invalid_params("URL sin host".to_string(), None))?;

    let port = url
        .port()
        .unwrap_or(if url.scheme() == "https" { 443 } else { 80 });

    let addrs: Vec<_> = lookup_host(format!("{host}:{port}"))
        .await
        .map_err(|e| {
            McpError::invalid_params(
                format!("error de resolución DNS para '{host}': {e}"),
                None,
            )
        })?
        .collect();

    if addrs.is_empty() {
        return Err(McpError::invalid_params(
            format!("no se pudo resolver la IP para '{host}'"),
            None,
        ));
    }

    for addr in &addrs {
        let ip = addr.ip();
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

/// Returns `true` if `ip` falls within a forbidden range.
///
/// Covers:
/// - IPv4: loopback (127.0.0.0/8), private (10/8, 172.16/12, 192.168/16),
///   link-local (169.254/16), CGNAT (100.64/10).
/// - IPv6: loopback (::1), unique-local (fc00::/7).
fn is_forbidden_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || is_cgnat(v4)
        }
        IpAddr::V6(v6) => v6.is_loopback() || is_ipv6_unique_local(v6),
    }
}

/// Returns `true` if `v4` is within the CGNAT range 100.64.0.0/10
/// (100.64.0.0 – 100.127.255.255).
fn is_cgnat(v4: &std::net::Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000
}

/// Returns `true` if `v6` is within the unique-local range fc00::/7
/// (fc00:: – fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff).
fn is_ipv6_unique_local(v6: &std::net::Ipv6Addr) -> bool {
    v6.segments()[0] & 0xfe00 == 0xfc00
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
