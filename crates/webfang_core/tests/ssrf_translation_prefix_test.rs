//! Regression probes for #1965 (A2 + A4 + invariants 1..4).
//!
//! Written as an independent probe — outside the crate, against the PUBLIC
//! `webfang_core` API — and kept as a regression test: it asserts that a
//! NAT64/6to4 translation prefix never inherits the loopback exemption,
//! that the literal loopback stays permitted under the flag, that the
//! v4-mapped form stays denied, that an allowlisted embedded address stays
//! reachable, and that the flat path's semantics are unchanged.
//!
//! The never-allowable table is reproduced by hand on purpose: the crate
//! keeps it `pub(crate)`, so an out-of-crate probe cannot read it — and the
//! point of the assertion is to compare the crate's dial-layer set against an
//! independent copy.

use std::net::IpAddr;

use webfang_core::domain::network_policy::{CidrBlock, NetworkPolicy, NetworkPolicyMode};
use webfang_core::domain::ssrf_guard::{
    ipv6_6to4_embedded_v4, ipv6_nat64_embedded_v4, is_always_denied_ip, is_forbidden_ip,
    is_forbidden_ip_with, is_forbidden_ip_with_policy, is_forbidden_resolved_address,
    is_permitted_loopback,
};

fn ip(s: &str) -> IpAddr {
    s.parse().expect("probe literal parses")
}

fn policy(cidrs: &[&str], hosts: &[&str]) -> NetworkPolicy {
    NetworkPolicy {
        mode: NetworkPolicyMode::Allowlist,
        allow_cidrs: cidrs
            .iter()
            .map(|c| CidrBlock::parse(c).expect("probe CIDR parses"))
            .collect(),
        allow_hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
    }
}

fn lan() -> NetworkPolicy {
    policy(&["10.0.0.0/8"], &["vllm.lan"])
}

// The never-allowable table, reproduced INDEPENDENTLY of the crate (the crate
// keeps it `pub(crate)`, so an outside probe cannot see it). Same entries as
// network_policy::NEVER_ALLOWABLE.
fn never_allowable() -> Vec<(IpAddr, u8, &'static str)> {
    vec![
        (ip("0.0.0.0"), 32, "unspecified"),
        (ip("169.254.0.0"), 16, "link-local (cloud metadata)"),
        (ip("224.0.0.0"), 4, "multicast"),
        (ip("240.0.0.0"), 4, "reserved"),
        (ip("255.255.255.255"), 32, "broadcast"),
        (ip("::"), 128, "unspecified"),
        (ip("fe80::"), 10, "link-local"),
        (ip("ff00::"), 8, "multicast"),
        (ip("2001:0::"), 32, "Teredo"),
        (ip("64:ff9b::"), 96, "NAT64 translation prefix"),
        (ip("2002::"), 16, "6to4 translation prefix"),
    ]
}

fn is_translation(v: &IpAddr) -> bool {
    match v {
        IpAddr::V6(v6) => {
            ipv6_nat64_embedded_v4(v6).is_some() || ipv6_6to4_embedded_v4(v6).is_some()
        },
        IpAddr::V4(_) => false,
    }
}

#[test]
fn probe_a2_translation_forms_of_loopback_are_denied_under_the_flag() {
    let p = lan();
    for form in ["64:ff9b::7f00:1", "2002:7f00:1::"] {
        let a = ip(form);
        let pol = is_forbidden_ip_with_policy(&a, true, &p);
        let res = is_forbidden_resolved_address(&a, "vllm.lan", true, &p);
        let res_off = is_forbidden_resolved_address(&a, "vllm.lan", false, &p);
        let pol_off = is_forbidden_ip_with_policy(&a, false, &p);
        println!(
            "A2 {form}: policy(flag=1)={pol} resolved(flag=1)={res} policy(flag=0)={pol_off} resolved(flag=0)={res_off}"
        );
        assert!(
            pol,
            "A2: {form} must be forbidden under the flag (policy path)"
        );
        assert!(
            res,
            "A2: {form} must be forbidden under the flag (resolved path)"
        );
        assert!(pol_off, "A2: {form} must stay forbidden with the flag off");
        assert!(res_off, "A2: {form} must stay forbidden with the flag off");
        // The literal-host path routes through the same policy verdict.
        assert!(
            webfang_core::domain::ssrf_guard::is_forbidden_literal_host_with_policy(form, true, &p)
        );
    }
}

#[test]
fn probe_invariant_1_literal_loopback_stays_permitted_under_the_flag() {
    let p = lan();
    for lit in ["127.0.0.1", "::1"] {
        let a = ip(lit);
        let pol = is_forbidden_ip_with_policy(&a, true, &p);
        let res = is_forbidden_resolved_address(&a, "vllm.lan", true, &p);
        let pol_off = is_forbidden_ip_with_policy(&a, false, &p);
        let res_off = is_forbidden_resolved_address(&a, "vllm.lan", false, &p);
        println!(
            "INV1 {lit}: policy(flag=1)={pol} resolved(flag=1)={res} policy(flag=0)={pol_off} resolved(flag=0)={res_off}"
        );
        assert!(!pol, "INV1: {lit} must stay permitted with the flag");
        assert!(!res, "INV1: {lit} must stay permitted with the flag");
        assert!(pol_off, "INV1: {lit} must stay denied without the flag");
        assert!(res_off, "INV1: {lit} must stay denied without the flag");
    }
}

#[test]
fn probe_invariant_2_v4_mapped_loopback_stays_denied() {
    let p = lan();
    for lit in ["::ffff:127.0.0.1", "::ffff:7f00:1"] {
        let a = ip(lit);
        let pol = is_forbidden_ip_with_policy(&a, true, &p);
        let res = is_forbidden_resolved_address(&a, "vllm.lan", true, &p);
        println!("INV2 {lit}: policy(flag=1)={pol} resolved(flag=1)={res}");
        assert!(pol, "INV2: {lit} must stay forbidden with the flag");
        assert!(res, "INV2: {lit} must stay forbidden with the flag");
    }
}

#[test]
fn probe_invariant_3_allowlisted_embedded_addresses_stay_allowed() {
    let p = lan();
    // RFC1918 embedded, allowlisted.
    for form in ["64:ff9b::a00:5", "2002:a00:5::"] {
        let a = ip(form);
        for flag in [false, true] {
            let pol = is_forbidden_ip_with_policy(&a, flag, &p);
            let res = is_forbidden_resolved_address(&a, "vllm.lan", flag, &p);
            println!("INV3 {form} flag={flag}: policy={pol} resolved={res}");
            assert!(!pol, "INV3: {form} must stay allowed (policy path)");
            assert!(!res, "INV3: {form} must stay allowed (resolved path)");
        }
        // Without the allowlist it must be denied.
        let none = policy(&[], &[]);
        assert!(is_forbidden_ip_with_policy(&a, true, &none));
        assert!(is_forbidden_resolved_address(&a, "vllm.lan", true, &none));
    }
    // CGNAT embedded, allowlisted (the explicit opt-in).
    let cgnat = policy(&["100.64.0.0/10"], &["vllm.lan"]);
    let a = ip("64:ff9b::6440:1");
    println!(
        "INV3c 64:ff9b::6440:1 (CGNAT 100.64.0.1): policy={} resolved={}",
        is_forbidden_ip_with_policy(&a, true, &cgnat),
        is_forbidden_resolved_address(&a, "vllm.lan", true, &cgnat)
    );
    assert!(!is_forbidden_ip_with_policy(&a, true, &cgnat));
    assert!(!is_forbidden_resolved_address(&a, "vllm.lan", true, &cgnat));
    // Cloud metadata embedded stays denied even when its range is allowlisted.
    let meta = policy(&["169.254.0.0/16"], &["meta.lan"]);
    for form in ["64:ff9b::a9fe:a9fe", "2002:a9fe:a9fe::"] {
        let a = ip(form);
        println!(
            "INV3b {form}: policy={} resolved={}",
            is_forbidden_ip_with_policy(&a, true, &meta),
            is_forbidden_resolved_address(&a, "meta.lan", true, &meta)
        );
        assert!(is_forbidden_ip_with_policy(&a, true, &meta));
        assert!(is_forbidden_resolved_address(&a, "meta.lan", true, &meta));
    }
}

#[test]
fn probe_invariant_4_flat_path_semantics_unchanged() {
    // (literal, expected forbidden)
    for lit in [
        "127.0.0.1",
        "::1",
        "::ffff:127.0.0.1",
        "64:ff9b::7f00:1",
        "2002:7f00:1::",
        "10.0.0.5",
        "169.254.169.254",
        "0.0.0.0",
        "::",
        "100.64.0.1",
        "255.255.255.255",
        "240.0.0.1",
    ] {
        let a = ip(lit);
        let got = is_forbidden_ip(&a);
        println!("INV4 is_forbidden_ip({lit}) = {got}");
        assert!(got, "INV4: {lit} must stay forbidden on the flat path");
    }
    for lit in ["8.8.8.8", "2606:4700::1111", "1.1.1.1"] {
        let a = ip(lit);
        let got = is_forbidden_ip(&a);
        println!("INV4 is_forbidden_ip({lit}) = {got}");
        assert!(!got, "INV4: {lit} must stay allowed on the flat path");
    }
    // OBSERVATION (pre-existing, unchanged by the diff): the FLAT path does not
    // deny multicast, while the policy path does (is_always_denied_ip).
    println!(
        "OBS is_forbidden_ip(224.0.0.1) = {}",
        is_forbidden_ip(&ip("224.0.0.1"))
    );
    // CGNAT is deny-by-default on the flat path but allowlistable, so it is
    // deliberately absent from the always-denied set.
    println!(
        "OBS is_forbidden_ip(100.64.0.0) = {}",
        is_forbidden_ip(&ip("100.64.0.0"))
    );
    // The flag variant without a policy: the permit opens the literal only.
    assert!(!is_forbidden_ip_with(&ip("127.0.0.1"), true));
    assert!(!is_forbidden_ip_with(&ip("::1"), true));
    assert!(is_forbidden_ip_with(&ip("127.0.0.1"), false));
    assert!(is_forbidden_ip_with(&ip("64:ff9b::7f00:1"), true));
    assert!(is_forbidden_ip_with(&ip("2002:7f00:1::"), true));
    // is_permitted_loopback covers exactly the two literals.
    assert!(is_permitted_loopback(&ip("127.0.0.1")));
    assert!(is_permitted_loopback(&ip("::1")));
    assert!(!is_permitted_loopback(&ip("127.0.0.2")));
    assert!(!is_permitted_loopback(&ip("::ffff:127.0.0.1")));
    // The flag must not change a public address verdict.
    let p = lan();
    assert!(!is_forbidden_ip_with_policy(&ip("8.8.8.8"), true, &p));
    assert!(!is_forbidden_ip_with_policy(&ip("8.8.8.8"), false, &p));
}

#[test]
fn probe_a4_never_allowable_relation() {
    let blanket = policy(&["0.0.0.0/0", "::/0"], &["probe.lan"]);
    let mut only_in_config = 0usize;
    for (range, len, class) in never_allowable() {
        let range = &range;
        let translation = is_translation(range);
        let always = is_always_denied_ip(range);
        println!(
            "A4 {class} {range}/{len}: always_denied={always} translation={translation} policy={} resolved={}",
            is_forbidden_ip_with_policy(range, false, &blanket),
            is_forbidden_resolved_address(range, "probe.lan", false, &blanket)
        );
        if translation {
            assert!(
                !always,
                "A4: {class} must be absent from the dial set on purpose"
            );
            only_in_config += 1;
        } else {
            assert!(always, "A4: {class} must be covered by the dial set");
        }
        assert!(
            is_forbidden_ip_with_policy(range, false, &blanket),
            "A4: {class} must stay unreachable under a blanket allowlist"
        );
        assert!(
            is_forbidden_ip_with_policy(range, true, &blanket),
            "A4: {class} must stay unreachable under a blanket allowlist (flag on)"
        );
        assert!(
            is_forbidden_resolved_address(range, "probe.lan", false, &blanket),
            "A4: {class} must stay unreachable under a blanket allowlist"
        );
        assert!(
            is_forbidden_resolved_address(range, "probe.lan", true, &blanket),
            "A4: {class} must stay unreachable under a blanket allowlist (flag on)"
        );
    }
    assert_eq!(
        only_in_config, 2,
        "A4: exactly the two translation prefixes differ"
    );
    assert!(!is_always_denied_ip(&ip("100.64.0.0")));
    assert!(is_forbidden_ip(&ip("100.64.0.0")));
}

#[test]
fn probe_boundaries_outside_the_named_scope() {
    let p = lan();
    // 0.0.0.0/8 family: the "this host" range. Pre-existing, flag-independent.
    for lit in ["0.0.0.1", "64:ff9b::0:1", "2002:0:1::"] {
        let a = ip(lit);
        let flat = is_forbidden_ip(&a);
        let pol = is_forbidden_ip_with_policy(&a, true, &p);
        let res = is_forbidden_resolved_address(&a, "vllm.lan", true, &p);
        println!("BOUND {lit}: flat={flat} policy(flag=1)={pol} resolved(flag=1)={res}");
    }
    // A NAT64 prefix length other than /96 (RFC 6052 allows /32../64).
    for lit in ["64:ff9b:1::7f00:1", "64:ff9b::1:7f00:1"] {
        let a = ip(lit);
        let flat = is_forbidden_ip(&a);
        let pol = is_forbidden_ip_with_policy(&a, true, &p);
        println!("BOUND {lit}: flat={flat} policy(flag=1)={pol}");
    }
}
