//! SSRF guard — pure deny-list policy plus the domain port (ADR-0012
//! sub-slice 3.C, issue #994).
//!
//! Defense is layered:
//!
//! 1. **Entry validation** — the MCP entry-point validator
//!    (`validate_url_no_ssrf`) resolves hostnames and checks every resolved
//!    address with [`is_forbidden_ip`] before any request leaves. This stays
//!    as fast-fail typed UX.
//! 2. **Connect-time enforcement** — the validating DNS resolver (defined in
//!    the infrastructure `ssrf` module, real GAI I/O stays there) is installed
//!    via `wreq::ClientBuilder::dns_resolver` through the [`SsrfGuard`] port on
//!    every scrape client and re-checks every DNS answer against
//!    [`is_forbidden_ip`]. Because redirects are followed inside the same
//!    client stack, each redirect hop's connection also resolves through this
//!    resolver, closing the gap where a hostname redirect target could reach an
//!    address that was never validated at entry (DNS rebinding / TOCTOU
//!    included).
//! 3. **Belt-and-suspenders literal guard** — [`redirect_policy`] still stops
//!    redirects whose target is a *literal* forbidden IP synchronously,
//!    before any resolution happens.
//!
//! All layers share [`is_forbidden_ip`] as the single deny list.
//!
//! # Third-party types in `domain/` — accepted deliberately
//!
//! This module puts two `wreq` types into the domain layer:
//! [`wreq::redirect::Policy`] (as the [`redirect_policy`] return type) and
//! [`wreq::ClientBuilder`] (in the [`SsrfGuard`] port signature). They join the
//! accepted-leak surface already disclosed by the
//! [`downloader_factory`](crate::domain::downloader_factory) precedent
//! (`wreq::cookie::Jar` and `tokio_util::sync::CancellationToken` there).
//!
//! This is a known, accepted leak, not an oversight: the ADR-0010 intra-crate
//! direction gate (`scripts/check_intra_crate_direction.sh`) only inspects
//! `crate::<layer>::…` paths, so it cannot see third-party leakage at all. A
//! future `domain`-owned client-builder newtype would close the gap; until
//! then, do not read a green gate as "the domain layer is framework-free".

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, OnceLock};

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
/// redirect targets given as hostnames are enforced by the connect-time
/// validating DNS resolver (see [`SsrfGuard`]) at connect time (every hop of a
/// followed redirect resolves through the same client stack), so this sync
/// guard only short-circuits *literal* IP targets before any resolution
/// happens. Non-forbidden attempts are delegated to [`Policy::default`] (which
/// is `Policy::limited(10)`), so the hop cap and loop protection of the
/// previous `Policy::limited(10)` are preserved.
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

/// Default concrete guard — the type lives in `domain`, its [`SsrfGuard`]
/// impl lives in the infrastructure `ssrf` module.
///
/// Type-here / impl-there placement (deliberate, not an oversight): the
/// registry fallback must be constructible inside `domain` — bypass paths
/// (CLI preflight, crawler discovery, sitemap discovery) never build a
/// `Container`, and the spec requires them to receive the FULL guard, not a
/// no-op stub. Applying the full guard requires the validating resolver, whose
/// construction is infrastructure I/O that a `domain` body may not reference
/// under the ADR-0010 gate. Same-crate impl placement solves it: Rust permits
/// the trait impl in any module of the defining crate, so the impl block sits
/// next to the resolver in the infrastructure `ssrf` module (infrastructure →
/// domain is the allowed direction) while this unit type stays
/// domain-constructible.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultSsrfGuard;

#[allow(missing_docs)]
pub(crate) mod sealed {
    #[allow(missing_docs)]
    pub trait Sealed {}
}

/// Domain port for the layered SSRF guard — sealed.
/// Only the domain `DefaultSsrfGuard` (impl in the infrastructure `ssrf`
/// module) may implement it.
pub trait SsrfGuard: Send + Sync + sealed::Sealed {
    /// Apply the full SSRF guard (redirect policy + validating resolver) to a
    /// client builder. Consuming: `wreq::ClientBuilder` is not `Clone`.
    fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder;
}

/// Process-wide SSRF guard instance, populated by the composition root
/// (`Container::new`) at startup and read by call sites that build HTTP
/// clients but cannot reach into the infrastructure layer.
///
/// The static is typed as the **domain** trait [`SsrfGuard`] so the domain
/// layer never names the infrastructure concrete — the trait impl for
/// [`DefaultSsrfGuard`] lives in the infrastructure `ssrf` module, reachable
/// only through this trait object.
static SSRF_GUARD: OnceLock<Arc<dyn SsrfGuard>> = OnceLock::new();

/// Install the process-wide SSRF guard. Idempotent: a second call is a
/// no-op when the first value is already set (keep-first, #996). Called by
/// the composition root (`Container::new`) at startup.
///
/// Arming-order caveat: the arming only wins the race if no client was built
/// earlier in the process — earlier builds materialize the fallback through
/// [`ssrf_guard`]. This cannot change behavior: the fallback and the armed
/// guard are behaviorally identical (`DefaultSsrfGuard` both ways), so arming
/// order only matters for test sentinels and future overrides.
pub fn set_ssrf_guard(guard: Arc<dyn SsrfGuard>) {
    let _ = SSRF_GUARD.set(guard);
}

/// Test-only probe: was the registry armed by a composition root?
///
/// The armed guard and the fallback are behaviorally identical
/// (`DefaultSsrfGuard` both ways), so [`ssrf_guard`] cannot distinguish
/// them; wiring tests need this probe to prove `Container::new` actually
/// calls [`set_ssrf_guard`] (strict-TDD RED without the arm).
#[cfg(test)]
pub(crate) fn ssrf_guard_armed() -> bool {
    SSRF_GUARD.get().is_some()
}

/// Read the process-wide SSRF guard. When no composition root has armed one,
/// falls back to the self-sufficient [`DefaultSsrfGuard`] (whose impl applies
/// the full guard — redirect policy + validating resolver) so paths that build
/// HTTP clients while bypassing `Container` (CLI preflight, crawler discovery,
/// sitemap discovery) are protected exactly like Container-wired clients.
///
/// The fallback is materialized once, at first use, and never replaced by a
/// later arming (the registry is keep-first).
#[must_use]
pub fn ssrf_guard() -> Arc<dyn SsrfGuard> {
    if let Some(guard) = SSRF_GUARD.get() {
        return guard.clone();
    }
    static FALLBACK: OnceLock<Arc<dyn SsrfGuard>> = OnceLock::new();
    FALLBACK
        .get_or_init(|| Arc::new(DefaultSsrfGuard) as Arc<dyn SsrfGuard>)
        .clone()
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

    // Registry semantics (new in sub-slice 3.C). The registry is process-global
    // and unresettable, so no test may assume it starts unarmed: nextest gives
    // each test its own process, but plain `cargo test` (the Coverage job)
    // shares one process, where a sibling that builds a Container arms it
    // first. Each registry test therefore captures the winner after its own arm
    // and asserts only the order- and race-independent invariant — once armed,
    // no later set replaces it (#996).
    #[test]
    fn ssrf_guard_port_is_object_safe_via_sealed() {
        struct FakeGuard;
        impl sealed::Sealed for FakeGuard {}
        impl SsrfGuard for FakeGuard {
            fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
                builder
            }
        }
        fn assert_dyn(_: &dyn SsrfGuard) {}
        let fake = FakeGuard;
        assert_dyn(&fake);
    }

    #[test]
    fn registry_is_keep_first() {
        struct FakeGuard1;
        struct FakeGuard2;
        impl sealed::Sealed for FakeGuard1 {}
        impl sealed::Sealed for FakeGuard2 {}
        impl SsrfGuard for FakeGuard1 {
            fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
                builder
            }
        }
        impl SsrfGuard for FakeGuard2 {
            fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
                builder
            }
        }

        let fake1: Arc<dyn SsrfGuard> = Arc::new(FakeGuard1);
        set_ssrf_guard(fake1);
        // Read after our own arm, so this is a registry value and never the
        // fallback: `FakeGuard1` when this test armed an unarmed registry,
        // or a sibling's guard in a shared `cargo test` process.
        let winner = ssrf_guard();
        set_ssrf_guard(Arc::new(FakeGuard2));
        assert!(
            Arc::ptr_eq(&ssrf_guard(), &winner),
            "a later set must not replace the already-armed guard (#996)"
        );
    }

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[test]
    fn registry_fallback_is_self_sufficient() {
        // Under a shared `cargo test` process a sibling may have armed the
        // registry first; either way the accessor must yield a guard that
        // can build a client.
        let guard = ssrf_guard();
        let client = guard.secure_client(wreq::Client::builder()).build();
        assert!(
            client.is_ok(),
            "unarmed accessor must yield a guard able to build a client"
        );
    }
}
