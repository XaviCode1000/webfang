//! Pure SSRF IP validation logic (no async, no DNS).
//!
//! The MCP entry-point validator (`validate_url_no_ssrf`) resolves hostnames and
//! checks every resolved address with [`is_forbidden_ip`]. Redirect policies,
//! however, run in a synchronous `wreq` callback where DNS resolution is
//! impossible, so they reuse [`redirect_policy`] to block redirects whose target
//! is a *literal* forbidden IP; hostname targets are only validated at entry.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use wreq::redirect::Policy;

/// Test-only escape hatch for the literal-IP redirect guard.
///
/// Mirrors the MCP crate's `WEBFANG_MCP_DISABLE_SSRF` convention: wiremock
/// listens on 127.0.0.1, so harnesses that exercise redirect chains need this
/// set before building clients. Production never sets it; the entry-level DNS
/// validator still blocks forbidden targets even when this is set, only the
/// synchronous redirect guard is lifted.
pub(crate) const DISABLE_REDIRECT_GUARD_ENV: &str = "WEBFANG_DISABLE_SSRF_REDIRECT_GUARD";

/// Returns `true` if `ip` falls within a forbidden range.
///
/// Covers:
/// - IPv4: loopback (127.0.0.0/8), private (10/8, 172.16/12, 192.168/16),
///   link-local (169.254/16), CGNAT (100.64.0.0/10).
/// - IPv6: loopback (::1), unspecified (::), unique-local (fc00::/7),
///   IPv4-mapped and IPv4-compatible (re-validated as IPv4).
#[must_use]
pub fn is_forbidden_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || is_cgnat(v4),
        IpAddr::V6(v6) => {
            // Native IPv6 special ranges first: `::1` and `::` also carry
            // IPv4-compatible representations (`::0.0.0.1` / `::0.0.0.0`) that
            // would otherwise re-validate as non-forbidden IPv4 addresses.
            if v6.is_loopback() || v6.is_unspecified() || is_ipv6_unique_local(v6) {
                return true;
            }
            // RFC 4291 §2.5.5.2: IPv4-mapped (`::ffff:a.b.c.d`) and the deprecated
            // IPv4-compatible (`::a.b.c.d`) form both surface through `to_ipv4()`;
            // both must be re-validated against the IPv4 deny list, otherwise any
            // forbidden IPv4 address can bypass the guard as `::ffff:x.x.x.x`.
            if let Some(v4) = v6.to_ipv4() {
                return is_forbidden_ip(&IpAddr::V4(v4));
            }
            false
        },
    }
}

/// Returns `true` if `v4` is within the CGNAT range 100.64.0.0/10
/// (100.64.0.0 – 100.127.255.255).
#[must_use]
pub fn is_cgnat(v4: &Ipv4Addr) -> bool {
    let octets = v4.octets();
    octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000
}

/// Returns `true` if `v6` is within the unique-local range fc00::/7
/// (fc00:: – fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff).
#[must_use]
pub fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    v6.segments()[0] & 0xfe00 == 0xfc00
}

/// Returns `true` if `host` is a literal IP address within a forbidden range.
///
/// Accepts bare IPv4 literals, bare IPv6 literals, and the bracketed form
/// produced by `http::Uri::host()` (e.g. `[::1]`). Hostnames return `false` —
/// synchronous redirect callbacks cannot resolve DNS; hostnames are validated
/// at entry by the async SSRF guard. Zone-id IPv6 literals (`[fe80::1%25eth0]`)
/// fail to parse as plain IPs and are treated as non-literals.
#[must_use]
pub fn is_forbidden_literal_host(host: &str) -> bool {
    let literal = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    literal
        .parse::<IpAddr>()
        .is_ok_and(|ip| is_forbidden_ip(&ip))
}

/// Redirect policy for all scrape clients: the default 10-hop limit with an
/// added SSRF guard that stops redirects targeting a literal forbidden IP.
///
/// Hostname targets are **not** blocked here (no DNS in a sync callback); they
/// are validated at entry by the async SSRF guard. Non-forbidden attempts are
/// delegated to [`Policy::default`] (which is `Policy::limited(10)`), so the
/// hop cap and loop protection of the previous `Policy::limited(10)` are
/// preserved.
///
/// Test harnesses that exercise redirect chains against wiremock (which binds
/// 127.0.0.1) can set `WEBFANG_DISABLE_SSRF_REDIRECT_GUARD=1` before building
/// clients; the entry-level DNS validator still protects production callers.
#[must_use]
pub fn redirect_policy() -> Policy {
    let base = Policy::default();
    let guard_enabled = std::env::var(DISABLE_REDIRECT_GUARD_ENV).is_err();
    Policy::custom(move |attempt| {
        if guard_enabled && attempt.uri.host().is_some_and(is_forbidden_literal_host) {
            tracing::warn!(
                target_uri = %attempt.uri,
                "Redirect to forbidden literal IP blocked (SSRF guard)"
            );
            attempt.stop()
        } else {
            base.redirect(attempt)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> IpAddr {
        s.parse().expect("test literals always parse")
    }

    #[test]
    fn ipv4_mapped_forms_are_revalidated_as_ipv4() {
        // IPv4-mapped (RFC 4291 §2.5.5.2)
        assert!(is_forbidden_ip(&addr("::ffff:127.0.0.1")));
        assert!(is_forbidden_ip(&addr("::ffff:10.0.0.1")));
        assert!(is_forbidden_ip(&addr("::ffff:169.254.169.254")));
        assert!(is_forbidden_ip(&addr("::ffff:100.64.0.1")));
        assert!(!is_forbidden_ip(&addr("::ffff:8.8.8.8")));
        // Deprecated IPv4-compatible form
        assert!(is_forbidden_ip(&addr("::127.0.0.1")));
    }

    #[test]
    fn native_v6_special_ranges_win_over_mapped_revalidation() {
        // `::1` and `::` also have IPv4-compatible projections; the native
        // loopback/unspecified checks must run first.
        assert!(is_forbidden_ip(&addr("::1")));
        assert!(is_forbidden_ip(&addr("::")));
        assert!(is_forbidden_ip(&addr("fc00::1")));
    }

    #[test]
    fn literal_host_classification() {
        assert!(is_forbidden_literal_host("127.0.0.1"));
        assert!(is_forbidden_literal_host("169.254.169.254"));
        assert!(is_forbidden_literal_host("[::1]"));
        assert!(is_forbidden_literal_host("[::ffff:127.0.0.1]"));
        assert!(!is_forbidden_literal_host("8.8.8.8"));
        assert!(!is_forbidden_literal_host("[2606:4700::1111]"));
        assert!(!is_forbidden_literal_host("example.com"));
        assert!(!is_forbidden_literal_host("127.0.0.1.nip.io"));
    }
}
