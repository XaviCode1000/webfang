//! Pure SSRF IP validation logic plus connect-time enforcement.
//!
//! Defense is layered:
//!
//! 1. **Entry validation** — the MCP entry-point validator
//!    (`validate_url_no_ssrf`) resolves hostnames and checks every resolved
//!    address with [`is_forbidden_ip`] before any request leaves. This stays
//!    as fast-fail typed UX.
//! 2. **Connect-time enforcement** — [`ValidatingResolver`] is installed via
//!    `wreq::ClientBuilder::dns_resolver` on every scrape client and re-checks
//!    every DNS answer against [`is_forbidden_ip`]. Because redirects are
//!    followed inside the same client stack, each redirect hop's connection
//!    also resolves through this resolver, closing the gap where a hostname
//!    redirect target could reach an address that was never validated at
//!    entry (DNS rebinding / TOCTOU included).
//! 3. **Belt-and-suspenders literal guard** — [`redirect_policy`] still stops
//!    redirects whose target is a *literal* forbidden IP synchronously,
//!    before any resolution happens.
//!
//! All layers share [`is_forbidden_ip`] as the single deny list.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use wreq::dns::{Addrs, Name, Resolve, Resolving};
use wreq::redirect::Policy;

/// Test-only escape hatch for the literal-IP redirect guard.
///
/// Mirrors the MCP crate's `WEBFANG_MCP_DISABLE_SSRF` convention: wiremock
/// listens on 127.0.0.1, so harnesses that exercise redirect chains need this
/// set before building clients. Production never sets it; the entry-level DNS
/// validator still blocks forbidden targets even when this is set, only the
/// synchronous redirect guard is lifted.
pub(crate) const DISABLE_REDIRECT_GUARD_ENV: &str = "WEBFANG_DISABLE_SSRF_REDIRECT_GUARD";

/// Test-only escape hatch for the connect-time validating DNS resolver.
///
/// Same rationale as [`DISABLE_REDIRECT_GUARD_ENV`]: wiremock binds 127.0.0.1,
/// which [`is_forbidden_ip`] rejects, so any harness driving real connections
/// through the production clients must set `WEBFANG_DISABLE_SSRF_RESOLVER=1`
/// before clients are built. Production never sets it.
pub(crate) const DISABLE_VALIDATING_RESOLVER_ENV: &str = "WEBFANG_DISABLE_SSRF_RESOLVER";

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
/// This is now **belt-and-suspenders**, not the primary hostname defense:
/// redirect targets given as hostnames are enforced by [`ValidatingResolver`]
/// at connect time (every hop of a followed redirect resolves through the
/// same client stack), so this sync guard only short-circuits *literal* IP
/// targets before any resolution happens. Non-forbidden attempts are
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

/// Error produced when a DNS answer violates SSRF policy.
///
/// The whole answer set is rejected fail-closed: a single forbidden address
/// among the returned records poisons the resolution, mirroring the
/// entry-level validator's semantics (never hand wreq a "safe subset" that an
/// attacker could pin connections to).
#[derive(Debug, thiserror::Error)]
#[error("DNS resolution of '{host}' returned forbidden address {ip} (SSRF)")]
pub struct ForbiddenResolutionError {
    /// Host whose answer set contained the forbidden address.
    pub host: String,
    /// The first forbidden address found in the answer set.
    pub ip: IpAddr,
}

/// Connect-time SSRF-enforcing DNS resolver installed on every scrape client.
///
/// Implements [`wreq::dns::Resolve`] using the same mechanism wreq uses by
/// default when hickory-dns is disabled — the GAI path: blocking
/// `getaddrinfo` through [`tokio::net::lookup_host`]. Every address in the
/// answer set is validated with [`is_forbidden_ip`]; if ANY address is
/// forbidden, the entire resolution fails (fail-closed, no safe-subset
/// filtering).
///
/// Because wreq follows redirects inside the same client stack
/// (`FollowRedirectLayer`), every redirect hop's connection resolves through
/// this resolver too — one choke point closes both owner-approved gaps:
/// hostname redirect targets and DNS-rebinding TOCTOU between entry
/// validation and connect.
///
/// The port in each resolved [`SocketAddr`] is `0`: per the `wreq::dns::Resolve`
/// contract, an explicit port in the request URI overrides it, and port 0 is
/// replaced with the scheme's conventional port otherwise. This mirrors
/// wreq's own `GaiResolver`, which resolves `(name, 0)`.
///
/// The escape hatch [`DISABLE_VALIDATING_RESOLVER_ENV`] is read **once, at
/// construction time** (stored as a plain `bool`): long-lived clients keep a
/// consistent policy for their whole lifetime (no half-disarmed states where
/// some pooled connections validate and others don't), and there is no
/// per-request env syscall on the resolve hot path.
#[derive(Debug, Clone)]
pub struct ValidatingResolver {
    validation_enabled: bool,
}

impl ValidatingResolver {
    /// Builds a resolver; validation state is captured from
    /// [`DISABLE_VALIDATING_RESOLVER_ENV`] at this moment.
    #[must_use]
    pub fn new() -> Self {
        Self {
            validation_enabled: std::env::var(DISABLE_VALIDATING_RESOLVER_ENV).is_err(),
        }
    }

    /// Resolves `host` through the GAI path (`getaddrinfo`), optionally
    /// validating every address in the answer set.
    ///
    /// # Errors
    ///
    /// Returns [`ForbiddenResolutionError`] when validating and any address
    /// in the answer set is forbidden, or the underlying `io::Error` if
    /// resolution itself fails.
    async fn gai_lookup(
        host: String,
        validate: bool,
    ) -> Result<Addrs, Box<dyn std::error::Error + Send + Sync>> {
        let addrs: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host((host.as_str(), 0)).await?.collect();

        if validate {
            // Fail-closed scan: any forbidden record rejects the whole answer.
            if let Some(offending) = addrs.iter().find(|addr| is_forbidden_ip(&addr.ip())) {
                let ip = offending.ip();
                tracing::warn!(
                    host = %host,
                    ip = %ip,
                    "Forbidden address in DNS answer rejected (SSRF validating resolver)"
                );
                return Err(Box::new(ForbiddenResolutionError { host, ip }));
            }
        }

        Ok(Box::new(addrs.into_iter()) as Addrs)
    }
}

impl Default for ValidatingResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolve for ValidatingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        // `Name` carries only the hostname (`Name::as_str`); the request URI's
        // explicit port overrides whatever we return, and port 0 falls back to
        // the scheme default — see trait docs in wreq.
        let fut = Self::gai_lookup(name.as_str().to_owned(), self.validation_enabled);
        Box::pin(fut)
    }
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

    /// Resolver tests. Determinism note: `tokio::net::lookup_host` resolves
    /// IP literals locally (no network, no resolver daemon), so every case
    /// here uses literal hosts only. A real-hostname DNS-failure case is
    /// deliberately omitted: `getaddrinfo` behavior for unresolvable names
    /// depends on the machine's resolver configuration and can block on
    /// unreachable DNS servers — not deterministic enough for CI.
    ///
    /// Env-var mutation is safe under nextest: each test runs in its own
    /// process, mirroring how `test_redirect_to_forbidden_literal_ip_is_stopped`
    /// already manipulates `DISABLE_REDIRECT_GUARD_ENV`.
    mod validating_resolver_tests {
        use super::*;
        use std::net::SocketAddr;
        use wreq::dns::{Name, Resolve};

        fn validation_on() -> ValidatingResolver {
            std::env::remove_var(DISABLE_VALIDATING_RESOLVER_ENV);
            ValidatingResolver::new()
        }

        async fn resolved_addrs(resolver: &ValidatingResolver, host: &str) -> Vec<SocketAddr> {
            resolver
                .resolve(Name::from(host))
                .await
                .expect("resolution succeeds")
                .collect()
        }

        #[tokio::test]
        async fn forbidden_ipv4_literal_hostname_is_rejected() {
            let resolver = validation_on();

            for host in ["127.0.0.1", "169.254.169.254"] {
                let outcome = resolver.resolve(Name::from(host)).await;
                assert!(
                    outcome.is_err(),
                    "loopback literal must be rejected at connect time: {host}"
                );
            }
        }

        #[tokio::test]
        async fn forbidden_ipv4_mapped_ipv6_is_rejected() {
            let resolver = validation_on();

            let outcome = resolver.resolve(Name::from("::ffff:127.0.0.1")).await;
            assert!(
                outcome.is_err(),
                "IPv4-mapped loopback must not bypass the validating resolver"
            );
        }

        #[tokio::test]
        async fn public_ip_literals_pass_through() {
            let resolver = validation_on();

            let v4 = resolved_addrs(&resolver, "8.8.8.8").await;
            assert_eq!(v4.len(), 1, "literal resolution yields one address");
            assert_eq!(v4[0].ip().to_string(), "8.8.8.8");

            let v6 = resolved_addrs(&resolver, "2606:4700::1111").await;
            assert_eq!(v6.len(), 1);
            assert_eq!(v6[0].ip().to_string(), "2606:4700::1111");
        }

        #[tokio::test]
        async fn rejection_error_names_the_offending_host_and_ip() {
            let resolver = validation_on();

            let err = match resolver.resolve(Name::from("127.0.0.1")).await {
                Err(err) => err,
                Ok(_) => panic!("loopback must be rejected"),
            };
            let rendered = err.to_string();
            assert!(
                rendered.contains("127.0.0.1"),
                "error must identify both host and forbidden ip: {rendered}"
            );
        }

        #[tokio::test]
        async fn env_bypass_allows_loopback_resolution() {
            std::env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "1");
            let resolver = ValidatingResolver::new();

            let addrs = resolved_addrs(&resolver, "127.0.0.1").await;
            assert_eq!(addrs[0].ip().to_string(), "127.0.0.1");
        }

        #[tokio::test]
        async fn disable_flag_is_read_once_at_construction() {
            // Construct with validation ON, then flip the escape hatch: the
            // already-built resolver must keep enforcing (the flag is read
            // once, at construction time, so long-lived clients cannot be
            // disarmed mid-flight by an env mutation).
            let resolver = validation_on();
            std::env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "1");

            let outcome = resolver.resolve(Name::from("127.0.0.1")).await;
            assert!(
                outcome.is_err(),
                "flag captured at construction must keep validation active"
            );
        }
    }
}
