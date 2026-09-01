//! Connect-time SSRF enforcement: the [`ValidatingResolver`] DNS guard and the
//! [`SsrfGuard`](crate::domain::ssrf_guard::SsrfGuard) trait impl.
//!
//! Pure policy (the deny-list functions and `redirect_policy`) lives in
//! `crate::domain::ssrf_guard`; this module re-exports it for backwards
//! compatibility (the `webfang_mcp` crate consumes it through this path) and
//! owns everything that performs real I/O:
//!
//! 1. **Entry validation** — the MCP entry-point validator
//!    (`validate_url_no_ssrf`) resolves hostnames and checks every resolved
//!    address with [`is_forbidden_ip`]
//!    before any request leaves. This stays as fast-fail typed UX.
//! 2. **Connect-time enforcement** — [`ValidatingResolver`] is installed via
//!    `wreq::ClientBuilder::dns_resolver` on every scrape client and re-checks
//!    every DNS answer against the shared deny list. Because redirects are
//!    followed inside the same client stack, each redirect hop's connection
//!    also resolves through this resolver, closing the gap where a hostname
//!    redirect target could reach an address that was never validated at
//!    entry (DNS rebinding / TOCTOU included).
//! 3. **Belt-and-suspenders literal guard** —
//!    [`redirect_policy`] still
//!    stops redirects whose target is a *literal* forbidden IP synchronously,
//!    before any resolution happens.
//!
//! Both layers 2 and 3 are applied together, in a single choke point, by
//! `impl SsrfGuard for DefaultSsrfGuard` below — the domain-owned
//! `DefaultSsrfGuard` type cannot reference infrastructure I/O, so the impl
//! lives here (infrastructure → domain is the allowed direction).
//!
//! Layer boundaries verified against wreq 6.0.0-rc.29: its HTTP connector
//! parses IP-literal hosts directly (`dns::SocketAddrs::try_parse`) and never
//! invokes a custom resolver for them (`src/conn/http.rs`, `HttpConnector::call`),
//! so [`ValidatingResolver`] only sees *hostname* connections — precisely the
//! gap left open by layers 1 and 3, with no overlap and no hole.
//!
//! All layers share
//! [`is_forbidden_ip`] as the
//! single deny list.

// Backwards-compatibility shim: the canonical home of the pure policy surface
// is `crate::domain::ssrf_guard` (ADR-0012 sub-slice 3.C). The `webfang_mcp`
// crate and infra-internal call sites consume these items through this path.
#[cfg(test)]
pub(crate) use crate::domain::ssrf_guard::DISABLE_REDIRECT_GUARD_ENV;
pub(crate) use crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV;
pub use crate::domain::ssrf_guard::{
    ipv6_6to4_embedded_v4, ipv6_nat64_embedded_v4, is_cgnat, is_forbidden_ip,
    is_forbidden_literal_host, is_ipv6_link_local, is_ipv6_teredo, is_ipv6_unique_local,
    is_reserved_v4, redirect_policy, DefaultSsrfGuard,
};

use std::net::IpAddr;
use wreq::dns::{Addrs, Name, Resolve, Resolving};

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

// Trait impl for the domain-owned `DefaultSsrfGuard` (type defined in
// `crate::domain::ssrf_guard`): the registry fallback must be constructible in
// domain, but applying the full guard requires `ValidatingResolver`, whose
// construction is infrastructure I/O a domain body may not reference. Same-crate
// impl placement keeps both halves satisfied (infrastructure → domain is the
// allowed direction). The resolver is constructed PER `secure_client` call, so
// the escape hatch is captured at client-build time exactly like the previous
// inline `dns_resolver(ValidatingResolver::new())` wiring.
impl crate::domain::ssrf_guard::sealed::Sealed for crate::domain::ssrf_guard::DefaultSsrfGuard {}

impl crate::domain::ssrf_guard::SsrfGuard for crate::domain::ssrf_guard::DefaultSsrfGuard {
    fn secure_client(&self, builder: wreq::ClientBuilder) -> wreq::ClientBuilder {
        builder
            .redirect(redirect_policy())
            .dns_resolver(ValidatingResolver::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    /// hold [`ENV_LOCK`] for its entire body, including the wiring proofs
    /// (which build clients through `DefaultSsrfGuard::secure_client`).
    // `await_holding_lock` is deliberate here: each #[tokio::test] runs on its
    // own current-thread runtime, so a guard held across `.await` can never be
    // contended by another task on the same runtime — and holding it for the
    // whole test body is exactly what serializes env mutation across test
    // THREADS (issue #926).
    #[allow(clippy::await_holding_lock)]
    mod validating_resolver_tests {
        use super::*;
        use std::net::SocketAddr;
        use wreq::dns::{Name, Resolve};

        use crate::domain::ssrf_guard::SsrfGuard as _;

        fn validation_on() -> (webfang_test_utils::EnvGuard, ValidatingResolver) {
            let guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
            (guard, ValidatingResolver::new())
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
            let (_guard, resolver) = validation_on();

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
            let (_guard, resolver) = validation_on();

            let outcome = resolver.resolve(Name::from("::ffff:127.0.0.1")).await;
            assert!(
                outcome.is_err(),
                "IPv4-mapped loopback must not bypass the validating resolver"
            );
        }

        #[tokio::test]
        async fn public_ip_literals_pass_through() {
            let (_guard, resolver) = validation_on();

            let v4 = resolved_addrs(&resolver, "8.8.8.8").await;
            assert_eq!(v4.len(), 1, "literal resolution yields one address");
            assert_eq!(v4[0].ip().to_string(), "8.8.8.8");

            let v6 = resolved_addrs(&resolver, "2606:4700::1111").await;
            assert_eq!(v6.len(), 1);
            assert_eq!(v6[0].ip().to_string(), "2606:4700::1111");
        }

        #[tokio::test]
        async fn rejection_error_names_the_offending_host_and_ip() {
            let (_guard, resolver) = validation_on();

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
            let _guard =
                webfang_test_utils::EnvGuard::with(&[(DISABLE_VALIDATING_RESOLVER_ENV, "1")]);
            let resolver = ValidatingResolver::new();

            let addrs = resolved_addrs(&resolver, "127.0.0.1").await;
            assert_eq!(addrs[0].ip().to_string(), "127.0.0.1");
        }

        #[tokio::test]
        async fn bypass_requires_exact_value_one() {
            // Any value other than the literal "1" (including "0", "true",
            // "yes") must NOT disarm the guard.
            let _guard =
                webfang_test_utils::EnvGuard::with(&[(DISABLE_VALIDATING_RESOLVER_ENV, "0")]);
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
            let (_guard, resolver) = validation_on();
            std::env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "1");

            let outcome = resolver.resolve(Name::from("127.0.0.1")).await;
            assert!(
                outcome.is_err(),
                "flag captured at construction must keep validation active"
            );
        }

        // Wiring proofs: a client built through the guard port
        // (`DefaultSsrfGuard::secure_client` — the same construction the
        // production sites consume) must enforce at connect time for
        // *hostname* targets. `localhost` resolves through getaddrinfo
        // (/etc/hosts → 127.0.0.1, ::1) without any network dependency, so
        // this stays deterministic in CI.
        //
        // The guard constructs the resolver per `secure_client` call, and
        // that construction reads `DISABLE_VALIDATING_RESOLVER_ENV`; each
        // proof therefore holds [`ENV_LOCK`] for its whole body so sibling
        // threads cannot race the env mutation (issue #926).
        #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
        #[tokio::test]
        async fn wired_client_rejects_hostname_resolving_to_loopback() {
            let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
            let client = crate::domain::ssrf_guard::DefaultSsrfGuard
                .secure_client(wreq::Client::builder())
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
            let _guard =
                webfang_test_utils::EnvGuard::with(&[(DISABLE_VALIDATING_RESOLVER_ENV, "1")]);
            let client = crate::domain::ssrf_guard::DefaultSsrfGuard
                .secure_client(wreq::Client::builder())
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

        // Unit proof for the guard port: through the `dyn SsrfGuard` trait
        // object (the shape production consumers hold), ONE choke-point call
        // applies the resolver layer — observable at connect time. The
        // redirect-policy layer shares the same impl body.
        #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
        #[tokio::test]
        async fn secure_client_applies_resolver_through_trait_object() {
            let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
            let guarded: std::sync::Arc<dyn crate::domain::ssrf_guard::SsrfGuard> =
                std::sync::Arc::new(crate::domain::ssrf_guard::DefaultSsrfGuard);
            let client = guarded
                .secure_client(wreq::Client::builder())
                .build()
                .expect("test client must build");

            let err = client
                .get("http://localhost:9/")
                .send()
                .await
                .expect_err("hostname resolving to loopback must fail at connect");
            assert!(
                format!("{err:?}").contains("ForbiddenResolutionError"),
                "guard trait object must apply the validating resolver: {err:?}"
            );
        }
    }
}
