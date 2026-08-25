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
//! Layer boundaries verified against wreq 6.0.0-rc.29: its HTTP connector
//! parses IP-literal hosts directly (`dns::SocketAddrs::try_parse`) and never
//! invokes a custom resolver for them (`src/conn/http.rs`, `HttpConnector::call`),
//! so [`ValidatingResolver`] only sees *hostname* connections — precisely the
//! gap left open by layers 1 and 3, with no overlap and no hole.
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
///   link-local (169.254/16), unspecified (0.0.0.0), broadcast
///   (255.255.255.255), reserved (240.0.0.0/4), CGNAT (100.64.0.0/10).
/// - IPv6: loopback (::1), unspecified (::), unique-local (fc00::/7),
///   unicast link-local (fe80::/10), IPv4-mapped and IPv4-compatible
///   (re-validated as IPv4), NAT64 well-known prefix 64:ff9b::/96 and
///   6to4 2002::/16 (embedded IPv4 re-validated as IPv4), Teredo
///   2001:0000::/32 (fail-closed).
#[must_use]
pub fn is_forbidden_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 0.0.0.0: `connect()` routes it to loopback on Linux/macOS.
                || v4.is_unspecified()
                || v4.is_broadcast()
                || is_reserved_v4(v4)
                || is_cgnat(v4)
        },
        IpAddr::V6(v6) => {
            // Native IPv6 special ranges first: `::1` and `::` also carry
            // IPv4-compatible representations (`::0.0.0.1` / `::0.0.0.0`) that
            // would otherwise re-validate as non-forbidden IPv4 addresses.
            if v6.is_loopback()
                || v6.is_unspecified()
                || is_ipv6_unique_local(v6)
                || is_ipv6_link_local(v6)
            {
                return true;
            }
            // RFC 4291 §2.5.5.2: IPv4-mapped (`::ffff:a.b.c.d`) and the deprecated
            // IPv4-compatible (`::a.b.c.d`) form both surface through `to_ipv4()`;
            // both must be re-validated against the IPv4 deny list, otherwise any
            // forbidden IPv4 address can bypass the guard as `::ffff:x.x.x.x`.
            if let Some(v4) = v6.to_ipv4() {
                return is_forbidden_ip(&IpAddr::V4(v4));
            }
            // Translation/encapsulation prefixes carry an embedded IPv4 address
            // that the socket stack dials directly on IPv6-only networks with
            // live NAT64 (mobile carriers, enterprises): `64:ff9b::a.b.c.d`
            // connects to `a.b.c.d` through the NAT64 gateway, and neither
            // native-range checks nor `to_ipv4()` ever see it. Re-validate every
            // embedded IPv4 against the same deny list.
            if let Some(v4) = ipv6_nat64_embedded_v4(v6).or_else(|| ipv6_6to4_embedded_v4(v6)) {
                return is_forbidden_ip(&IpAddr::V4(v4));
            }
            // Teredo (RFC 4380, 2001:0000::/32): the client IPv4/port are
            // XOR-obfuscated against the attacker-controllable relay's view of
            // them, so embedded-address re-validation cannot be trusted. Fail
            // closed over the whole range.
            if is_ipv6_teredo(v6) {
                return true;
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

/// Returns `true` if `v4` is within the reserved range 240.0.0.0/4,
/// excluding broadcast (255.255.255.255). Mirrors std's still-unstable
/// `Ipv4Addr::is_reserved` (#27709) with the same semantics.
#[must_use]
pub fn is_reserved_v4(v4: &Ipv4Addr) -> bool {
    v4.octets()[0] & 0b1111_0000 == 0b1111_0000 && !v4.is_broadcast()
}

/// Returns `true` if `v6` is within the unique-local range fc00::/7
/// (fc00:: – fdff:ffff:ffff:ffff:ffff:ffff:ffff:ffff).
#[must_use]
pub fn is_ipv6_unique_local(v6: &Ipv6Addr) -> bool {
    v6.segments()[0] & 0xfe00 == 0xfc00
}

/// Returns `true` if `v6` is within the unicast link-local range fe80::/10
/// (fe80:: – febf:ffff:...), symmetric with the IPv4 link-local denial
/// (169.254/16, which covers the cloud-metadata endpoint 169.254.169.254).
#[must_use]
pub fn is_ipv6_link_local(v6: &Ipv6Addr) -> bool {
    v6.segments()[0] & 0xffc0 == 0xfe80
}

/// Returns the IPv4 address embedded in a NAT64-translated address when
/// `v6` falls within the well-known prefix 64:ff9b::/96 (RFC 6052 §2.1);
/// `None` otherwise. The embedded IPv4 occupies segments\[6..8\].
#[must_use]
pub fn ipv6_nat64_embedded_v4(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    if segments[0] != 0x0064
        || segments[1] != 0xff9b
        || !segments[2..6].iter().all(|&segment| segment == 0)
    {
        return None;
    }
    Some(Ipv4Addr::new(
        (segments[6] >> 8) as u8,
        segments[6] as u8,
        (segments[7] >> 8) as u8,
        segments[7] as u8,
    ))
}

/// Returns the IPv4 address encapsulated in a 6to4 address (RFC 3056) when
/// `v6` falls within 2002::/16; `None` otherwise. The embedded IPv4 bytes
/// occupy the high byte of segments\[1\], the low byte of segments\[1\], the
/// high byte of segments\[2\], and the low byte of segments\[2\] — e.g.
/// `2002:7f00:1::` embeds 127.0.0.1.
#[must_use]
pub fn ipv6_6to4_embedded_v4(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    if segments[0] != 0x2002 {
        return None;
    }
    Some(Ipv4Addr::new(
        (segments[1] >> 8) as u8,
        segments[1] as u8,
        (segments[2] >> 8) as u8,
        segments[2] as u8,
    ))
}

/// Returns `true` if `v6` falls within the Teredo range 2001:0000::/32
/// (RFC 4380).
#[must_use]
pub fn is_ipv6_teredo(v6: &Ipv6Addr) -> bool {
    let segments = v6.segments();
    segments[0] == 0x2001 && segments[1] == 0x0000
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
/// The port in each resolved `SocketAddr` is `0`: per the `wreq::dns::Resolve`
/// contract, an explicit port in the request URI overrides it, and port 0 is
/// replaced with the scheme's conventional port otherwise. This mirrors
/// wreq's own `GaiResolver`, which resolves `(name, 0)`.
///
/// The escape hatch `DISABLE_VALIDATING_RESOLVER_ENV` is read **once, at
/// construction time** (stored as a plain `bool`): long-lived clients keep a
/// consistent policy for their whole lifetime (no half-disarmed states where
/// some pooled connections validate and others don't), and there is no
/// per-request env syscall on the resolve hot path.
///
/// Note: wreq short-circuits IP-literal hosts before calling any custom
/// resolver, so this type only ever sees hostname connections; literal-IP
/// targets are covered by entry validation and [`redirect_policy`].
#[derive(Debug, Clone)]
pub struct ValidatingResolver {
    validation_enabled: bool,
}

impl ValidatingResolver {
    /// Builds a resolver; validation state is captured from
    /// `DISABLE_VALIDATING_RESOLVER_ENV` at this moment. Only the exact
    /// value `"1"` disarms the guard — any other value (including `"0"`,
    /// `"true"`, `"yes"`) keeps validation active.
    #[must_use]
    pub fn new() -> Self {
        Self {
            validation_enabled: std::env::var(DISABLE_VALIDATING_RESOLVER_ENV).as_deref()
                != Ok("1"),
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
            Self::fail_closed_scan(&host, &addrs)?;
        }

        Ok(Box::new(addrs.into_iter()) as Addrs)
    }

    /// Fail-closed answer-set scan: ANY forbidden record rejects the whole
    /// resolution (no safe-subset filtering), and an EMPTY answer set is an
    /// error too — an empty set can never yield a connectable address, so it
    /// must not masquerade as success.
    ///
    /// Pure function over the answer slice so the empty-set edge is unit-
    /// testable without depending on nondeterministic `getaddrinfo` behavior.
    fn fail_closed_scan(
        host: &str,
        addrs: &[std::net::SocketAddr],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let Some(offending) = addrs.iter().find(|addr| is_forbidden_ip(&addr.ip())) else {
            if addrs.is_empty() {
                tracing::warn!(host = %host, "Empty DNS answer rejected (SSRF validating resolver)");
                return Err(Box::new(std::io::Error::other(format!(
                    "empty DNS answer for '{host}'"
                ))));
            }
            return Ok(());
        };
        let ip = offending.ip();
        tracing::warn!(
            host = %host,
            ip = %ip,
            "Forbidden address in DNS answer rejected (SSRF validating resolver)"
        );
        Err(Box::new(ForbiddenResolutionError {
            host: host.to_owned(),
            ip,
        }))
    }
}

impl ValidatingResolver {
    /// Builds a resolver with an explicitly chosen validation mode.
    ///
    /// Test-only seam: wiring proofs construct clients deterministically
    /// without reading process-global environment state (see the env-race
    /// note in `validating_resolver_tests`).
    #[cfg(test)]
    fn new_with_validation(validation_enabled: bool) -> Self {
        Self { validation_enabled }
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
    fn unspecified_broadcast_reserved_v4_is_forbidden() {
        // 0.0.0.0 routes to loopback on Linux/macOS — must never be allowed.
        assert!(is_forbidden_ip(&addr("0.0.0.0")));
        // The IPv4-mapped form falls into the recursive IPv4 arm: it MUST be
        // forbidden too, otherwise it bypasses the guard as a "public" v6.
        assert!(is_forbidden_ip(&addr("::ffff:0.0.0.0")));
        assert!(is_forbidden_literal_host("[::ffff:0.0.0.0]"));
        assert!(is_forbidden_literal_host("0.0.0.0"));
        // Reserved 240.0.0.0/4 and broadcast.
        assert!(is_forbidden_ip(&addr("240.0.0.1")));
        assert!(is_forbidden_ip(&addr("255.255.255.255")));
    }

    #[test]
    fn translated_and_encapsulated_ranges_are_revalidated_as_ipv4() {
        // Finding R3-1 exact trigger addresses.
        assert!(is_forbidden_ip(&addr("64:ff9b::7f00:1"))); // NAT64 → 127.0.0.1
        assert!(is_forbidden_ip(&addr("2002:7f00:1::"))); // 6to4 → 127.0.0.1
                                                          // Loopback/private/metadata/CGNAT embedded through each range.
        assert!(is_forbidden_ip(&addr("64:ff9b::a9fe:a9fe"))); // 169.254.169.254
        assert!(is_forbidden_ip(&addr("64:ff9b::a00:1"))); // 10.0.0.1
        assert!(is_forbidden_ip(&addr("64:ff9b::6440:1"))); // 100.64.0.1
        assert!(is_forbidden_ip(&addr("64:ff9b::ffff:ffff"))); // broadcast
        assert!(is_forbidden_ip(&addr("2002:a9fe:a9fe::"))); // 169.254.169.254
        assert!(is_forbidden_ip(&addr("2002:a00:1::"))); // 10.0.0.1
    }

    #[test]
    fn teredo_range_fails_closed() {
        // 2001:0000::/32: the client IPv4 is XOR-obfuscated and the relay
        // is attacker-controllable, so the whole range fails closed.
        assert!(is_forbidden_ip(&addr("2001:0::1")));
        assert!(is_forbidden_ip(&addr("2001:0:abcd:ef01:5678:5678:7f00:1")));
    }

    #[test]
    fn public_addresses_near_translation_prefixes_stay_allowed() {
        // Real public addresses remain allowed.
        assert!(!is_forbidden_ip(&addr("2606:4700::1111")));
        assert!(!is_forbidden_ip(&addr("2001:4860:4860::8888")));
        assert!(!is_forbidden_ip(&addr("2620:0:ccc::2")));
        // Public IPv4 embedded through translation ranges remains allowed.
        assert!(!is_forbidden_ip(&addr("64:ff9b::808:808"))); // 8.8.8.8
        assert!(!is_forbidden_ip(&addr("2002:808:808::"))); // 8.8.8.8
                                                            // Prefix look-alikes outside the ranges.
        assert!(!is_forbidden_ip(&addr("2001:db8::1"))); // not 2001:0::/32
        assert!(!is_forbidden_ip(&addr("64:ff9b:1::1"))); // outside 64:ff9b::/96
        assert!(!is_forbidden_ip(&addr("2002:100:1::"))); // 6to4 → 1.0.0.1 (public)
    }

    #[test]
    fn ipv6_unicast_link_local_is_forbidden() {
        // fe80::/10 spans fe80:: .. febf:ffff:... — symmetric with the
        // IPv4 link-local denial (169.254/16).
        assert!(is_forbidden_ip(&addr("fe80::1")));
        assert!(is_forbidden_ip(&addr("febf::ffff")));
        // fec0::/10 (deprecated site-local) is NOT link-local; outside scope.
        assert!(!is_forbidden_ip(&addr("fec0::1")));
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
    /// Env-var hermeticity contract: `cargo test` (including the Coverage
    /// job's `cargo llvm-cov` run) executes all tests of a binary in ONE
    /// process on parallel threads sharing process-global environment
    /// state — only nextest isolates per-test processes. Every test that
    /// reads or mutates `DISABLE_VALIDATING_RESOLVER_ENV` must therefore
    /// hold [`ENV_LOCK`] for its entire body, and wiring proofs must use
    /// [`ValidatingResolver::new_with_validation`] instead of env at all.
    // `await_holding_lock` is deliberate here: each #[tokio::test] runs on its
    // own current-thread runtime, so a guard held across `.await` can never be
    // contended by another task on the same runtime — and holding it for the
    // whole test body is exactly what serializes env mutation across test
    // THREADS (issue #926).
    #[allow(clippy::await_holding_lock)]
    mod validating_resolver_tests {
        use super::*;
        use std::net::SocketAddr;
        use std::sync::{Mutex, MutexGuard};
        use wreq::dns::{Name, Resolve};

        /// Serializes process-global env mutation across parallel threads
        /// (`cargo test` shares one process env; see the module contract
        /// comment above). Poison recovery keeps the suite running after a
        /// panicking sibling.
        static ENV_LOCK: Mutex<()> = Mutex::new(());

        fn env_guard() -> MutexGuard<'static, ()> {
            match ENV_LOCK.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            }
        }

        fn validation_on(_guard: &MutexGuard<'static, ()>) -> ValidatingResolver {
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
            let guard = env_guard();
            let resolver = validation_on(&guard);

            for host in ["127.0.0.1", "169.254.169.254", "0.0.0.0"] {
                let outcome = resolver.resolve(Name::from(host)).await;
                assert!(
                    outcome.is_err(),
                    "forbidden literal must be rejected at connect time: {host}"
                );
            }
        }

        #[test]
        fn fail_closed_scan_rejects_empty_answer_set() {
            // An empty DNS answer can never yield a connectable address:
            // fail closed regardless of which (if any) record was expected.
            let outcome = ValidatingResolver::fail_closed_scan("empty.test", &[]);
            assert!(outcome.is_err(), "empty answer set must fail closed");
        }

        #[tokio::test]
        async fn forbidden_ipv4_mapped_ipv6_is_rejected() {
            let guard = env_guard();
            let resolver = validation_on(&guard);

            let outcome = resolver.resolve(Name::from("::ffff:127.0.0.1")).await;
            assert!(
                outcome.is_err(),
                "IPv4-mapped loopback must not bypass the validating resolver"
            );
        }

        #[tokio::test]
        async fn public_ip_literals_pass_through() {
            let guard = env_guard();
            let resolver = validation_on(&guard);

            let v4 = resolved_addrs(&resolver, "8.8.8.8").await;
            assert_eq!(v4.len(), 1, "literal resolution yields one address");
            assert_eq!(v4[0].ip().to_string(), "8.8.8.8");

            let v6 = resolved_addrs(&resolver, "2606:4700::1111").await;
            assert_eq!(v6.len(), 1);
            assert_eq!(v6[0].ip().to_string(), "2606:4700::1111");
        }

        #[tokio::test]
        async fn rejection_error_names_the_offending_host_and_ip() {
            let guard = env_guard();
            let resolver = validation_on(&guard);

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
            let _guard = env_guard();
            std::env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "1");
            let resolver = ValidatingResolver::new();

            let addrs = resolved_addrs(&resolver, "127.0.0.1").await;
            assert_eq!(addrs[0].ip().to_string(), "127.0.0.1");
        }

        #[tokio::test]
        async fn bypass_requires_exact_value_one() {
            // Any value other than the literal "1" (including "0", "true",
            // "yes") must NOT disarm the guard.
            let _guard = env_guard();
            std::env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "0");
            let resolver = ValidatingResolver::new();

            let outcome = resolver.resolve(Name::from("127.0.0.1")).await;
            assert!(
                outcome.is_err(),
                r#"WEBFANG_DISABLE_SSRF_RESOLVER=0 must keep validation active"#
            );
        }

        #[tokio::test]
        async fn disable_flag_is_read_once_at_construction() {
            // Construct with validation ON, then flip the escape hatch: the
            // already-built resolver must keep enforcing (the flag is read
            // once, at construction time, so long-lived clients cannot be
            // disarmed mid-flight by an env mutation).
            let guard = env_guard();
            let resolver = validation_on(&guard);
            std::env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "1");

            let outcome = resolver.resolve(Name::from("127.0.0.1")).await;
            assert!(
                outcome.is_err(),
                "flag captured at construction must keep validation active"
            );
        }

        // Wiring proofs: a client built exactly like the production sites
        // (`redirect_policy()` + `dns_resolver(ValidatingResolver)`) must
        // enforce at connect time for *hostname* targets. `localhost`
        // resolves through getaddrinfo (/etc/hosts → 127.0.0.1, ::1) without
        // any network dependency, so this stays deterministic in CI.
        //
        // These build the resolver via `new_with_validation` instead of
        // reading process-global env: under plain `cargo test` (Coverage's
        // llvm-cov run) sibling tests mutate `DISABLE_VALIDATING_RESOLVER_ENV`
        // on parallel threads, and an env-read here raced that mutation into
        // a silently disarmed resolver (issue #926).
        #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
        #[tokio::test]
        async fn wired_client_rejects_hostname_resolving_to_loopback() {
            let client = wreq::Client::builder()
                .redirect(redirect_policy())
                .dns_resolver(ValidatingResolver::new_with_validation(true))
                .build()
                .expect("test client must build");

            let err = client
                .get("http://localhost:9/")
                .send()
                .await
                .expect_err("hostname resolving to loopback must fail at connect");
            assert!(
                format!("{err:?}").contains("ForbiddenResolutionError"),
                "failure must come from the SSRF resolver, not the network: {err:?}"
            );
        }

        #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
        #[tokio::test]
        async fn wired_client_reaches_connect_when_validation_disabled() {
            let client = wreq::Client::builder()
                .redirect(redirect_policy())
                .dns_resolver(ValidatingResolver::new_with_validation(false))
                .build()
                .expect("test client must build");

            // Port 9 (discard): nothing listens; resolution succeeds and the
            // failure must be a CONNECT error, never an SSRF rejection.
            let err = client
                .get("http://localhost:9/")
                .send()
                .await
                .expect_err("nothing listens on port 9");
            assert!(
                !format!("{err:?}").contains("forbidden address"),
                "bypassed resolver must not reject: {err:?}"
            );
        }
    }
}
