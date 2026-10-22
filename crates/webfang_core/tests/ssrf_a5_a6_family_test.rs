//! Independent regression probes for #1965 (A5 + A6) and its two follow-ups
//! (#2231, #2232): the "this network on this host" family 0.0.0.0/8 and the
//! NAT64 Network-Specific Prefixes must be denied on EVERY path, and so must
//! the ISATAP encapsulation (RFC 5214 §6.1) and the deprecated IPv4-compatible
//! spelling — while the legitimate translation forms and the legitimate
//! allowlist cases must keep working.
//!
//! Written out-of-crate against the PUBLIC `webfang_core` API on purpose:
//! the probe must not depend on the crate's internal helpers, so it proves
//! the guard's contract rather than its implementation.

use std::net::IpAddr;

use webfang_core::domain::network_policy::{CidrBlock, NetworkPolicy, NetworkPolicyMode};
use webfang_core::domain::ssrf_guard::{
    is_always_denied_ip, is_forbidden_ip, is_forbidden_ip_with_policy,
    is_forbidden_literal_host_with_policy, is_forbidden_resolved_address,
};

fn ip(s: &str) -> IpAddr {
    s.parse().expect("probe literal parses")
}

/// A policy built IN CODE, bypassing serde validation: the guard must hold
/// even when the allowlist carries the very range under test.
fn hand_built(cidrs: &[&str], hosts: &[&str]) -> NetworkPolicy {
    NetworkPolicy {
        mode: NetworkPolicyMode::Allowlist,
        allow_cidrs: cidrs
            .iter()
            .map(|c| CidrBlock::parse(c).expect("probe CIDR parses"))
            .collect(),
        allow_hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
    }
}

/// A5 (#1965): 0.0.0.0/8 is denied as a FAMILY on every path, with the
/// loopback flag on and off, and a hand-built allowlist of the family itself
/// does not reach it (the always-denied set denies FIRST).
#[test]
fn this_network_family_is_denied_on_every_path() {
    let family = hand_built(&["0.0.0.0/8"], &["probe.lan"]);
    for literal in [
        "0.0.0.0",
        "0.0.0.1",
        "0.0.0.2",
        "0.0.0.255",
        "0.1.0.1",
        "0.1.2.3",
        "0.255.255.254",
        "0.255.255.255",
    ] {
        let addr = ip(literal);
        assert!(is_forbidden_ip(&addr), "{literal}: flat path");
        assert!(is_always_denied_ip(&addr), "{literal}: dial-layer set");
        for flag in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&addr, flag, &family),
                "{literal}: policy path (allow_loopback={flag})"
            );
            assert!(
                is_forbidden_resolved_address(&addr, "probe.lan", flag, &family),
                "{literal}: resolved path (allow_loopback={flag})"
            );
        }
    }
}

/// A5 (#1965): the TRANSLATED forms of the family are denied too — the
/// translation prefixes delegate their verdict to the embedded IPv4.
#[test]
fn translated_this_network_forms_are_denied_on_every_path() {
    let family = hand_built(&["0.0.0.0/8"], &["probe.lan"]);
    for literal in ["64:ff9b::0:1", "64:ff9b::0:0:1", "2002:0:1::"] {
        let addr = ip(literal);
        assert!(is_forbidden_ip(&addr), "{literal}: flat path");
        for flag in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&addr, flag, &family),
                "{literal}: policy path (allow_loopback={flag})"
            );
            assert!(
                is_forbidden_resolved_address(&addr, "probe.lan", flag, &family),
                "{literal}: resolved path (allow_loopback={flag})"
            );
        }
    }
}

/// A5 (#1965): the IPv4-mapped spelling of the family is denied on every
/// path — the dial-layer set normalizes the mapped form to IPv4.
#[test]
fn ipv4_mapped_this_network_form_is_denied_on_every_path() {
    let family = hand_built(&["0.0.0.0/8"], &["probe.lan"]);
    for literal in ["::ffff:0.0.0.1", "::ffff:0.0.0.2"] {
        let addr = ip(literal);
        assert!(is_forbidden_ip(&addr), "{literal}: flat path");
        assert!(is_always_denied_ip(&addr), "{literal}: dial-layer set");
        for flag in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&addr, flag, &family),
                "{literal}: policy path (allow_loopback={flag})"
            );
            assert!(
                is_forbidden_resolved_address(&addr, "probe.lan", flag, &family),
                "{literal}: resolved path (allow_loopback={flag})"
            );
        }
    }
}

/// A5 (#1965): every URL spelling of 0.0.0.1 that the literal parser
/// accepts must be denied — `http://1/` and `http://0.1/` are real URL
/// forms of this-network address.
#[test]
fn this_network_literal_spellings_are_denied() {
    let family = hand_built(&["0.0.0.0/8"], &["probe.lan"]);
    for literal in [
        "0.0.0.0", "0.0.0.1", "0.1", "1", "0x1", "00.0.0.1", "0.0.0.1.",
    ] {
        for flag in [false, true] {
            assert!(
                is_forbidden_literal_host_with_policy(literal, flag, &family),
                "{literal}: literal path (allow_loopback={flag})"
            );
        }
    }
}

/// A6 (#1965): a NAT64 Network-Specific Prefix cannot be decoded (RFC 6052
/// §2.2 does not encode where the embedded IPv4 sits), so the whole
/// non-canonical space fails closed on every path.
#[test]
fn non_canonical_nat64_is_denied_on_every_path() {
    let nsp = hand_built(&["64:ff9b:1::/64"], &["probe.lan"]);
    for literal in [
        "64:ff9b:1::7f00:1",
        "64:ff9b:1::a00:1",
        "64:ff9b:0:1::1",
        "64:ff9b:1:0:0:0:7f00:1",
        "64:ff9b:ffff:ffff::1",
        "64:ff9b:8000::1",
        "64:ff9b:1::1",
    ] {
        let addr = ip(literal);
        assert!(is_forbidden_ip(&addr), "{literal}: flat path");
        assert!(is_always_denied_ip(&addr), "{literal}: dial-layer set");
        for flag in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&addr, flag, &nsp),
                "{literal}: policy path (allow_loopback={flag})"
            );
            assert!(
                is_forbidden_resolved_address(&addr, "probe.lan", flag, &nsp),
                "{literal}: resolved path (allow_loopback={flag})"
            );
        }
    }
}

/// A6 (#1965) positive control: the legitimate translation forms must keep
/// working. The canonical Well-Known Prefix of a PUBLIC address is the
/// standard NAT64 path for IPv6-only deployments; the 6to4 form of a public
/// address is the same shape. Both must stay ALLOWED.
#[test]
fn legitimate_translation_forms_stay_allowed() {
    for literal in ["64:ff9b::8.8.8.8", "64:ff9b::0:0:8.8.8.8", "2002:808:808::"] {
        let addr = ip(literal);
        assert!(!is_forbidden_ip(&addr), "{literal}: flat path");
        assert!(!is_always_denied_ip(&addr), "{literal}: dial-layer set");
        for flag in [false, true] {
            assert!(
                !is_forbidden_ip_with_policy(&addr, flag, &hand_built(&[], &[])),
                "{literal}: policy path (allow_loopback={flag})"
            );
        }
    }
}

/// A6 (#1965) positive control: ordinary public addresses stay allowed.
#[test]
fn ordinary_public_addresses_stay_allowed() {
    for literal in [
        "8.8.8.8",
        "1.1.1.1",
        "93.184.216.34",
        "2001:4860:4860::8888",
        "2606:4700:4700::1111",
        "2001:db8::1",
    ] {
        let addr = ip(literal);
        assert!(!is_forbidden_ip(&addr), "{literal}: flat path");
        assert!(!is_always_denied_ip(&addr), "{literal}: dial-layer set");
    }
}

/// A2 (#2185) must be untouched: the canonical Well-Known Prefix of a
/// loopback or private address stays FORBIDDEN, and the loopback flag does
/// not travel through a translation prefix.
#[test]
fn canonical_nat64_of_forbidden_addresses_stays_forbidden() {
    for literal in ["64:ff9b::7f00:1", "64:ff9b::a00:1", "2002:7f00:1::"] {
        let addr = ip(literal);
        assert!(is_forbidden_ip(&addr), "{literal}: flat path");
        for flag in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&addr, flag, &hand_built(&[], &[])),
                "{literal}: policy path (allow_loopback={flag})"
            );
            assert!(
                is_forbidden_resolved_address(&addr, "probe.lan", flag, &hand_built(&[], &[])),
                "{literal}: resolved path (allow_loopback={flag})"
            );
        }
    }
}

/// The legitimate allowlist case must keep working: a policy that allows
/// 10.0.0.0/8 still reaches a 10.0.0.1 address.
#[test]
fn legitimate_allowlist_still_reaches_the_allowed_network() {
    let lan = hand_built(&["10.0.0.0/8"], &["vllm.lan"]);
    let addr = ip("10.0.0.1");
    for flag in [false, true] {
        assert!(
            !is_forbidden_ip_with_policy(&addr, flag, &lan),
            "10.0.0.1: policy path (allow_loopback={flag})"
        );
        assert!(
            !is_forbidden_resolved_address(&addr, "vllm.lan", flag, &lan),
            "10.0.0.1: resolved path (allow_loopback={flag})"
        );
    }
}

/// #2232 (ISATAP, RFC 5214 §6.1): the ATOP identifier `0x5efe` sits at
/// segments\[5\] and the encapsulated IPv4 occupies the LOW 32 bits
/// (segments\[6..8\]) — the same fixed position the NAT64 Well-Known Prefix
/// uses, which is exactly why ISATAP can be decoded where a NAT64
/// Network-Specific Prefix cannot.
///
/// The embedded address MUST be re-validated on every path, exactly like
/// the NAT64/6to4 forms: `::5efe:0:1` reaches 0.0.0.1, `::5efe:7f00:1`
/// reaches 127.0.0.1 and `::5efe:a00:1` reaches 10.0.0.1.
///
/// The allowlist used here covers the ISATAP SPACE (`::/64`) and nothing
/// else: a blanket over the encapsulation must never reach the embedded
/// address, exactly like a blanket over the NAT64 prefix.
#[test]
fn isatap_embedded_forbidden_addresses_are_denied_on_every_path() {
    let blanket = hand_built(&["::/64"], &["probe.lan"]);
    for literal in [
        "::5efe:0:1",       // 0.0.0.1 — the "this network" family
        "::5efe:7f00:1",    // 127.0.0.1 — loopback, flag-independent
        "::5efe:a00:1",     // 10.0.0.1 — RFC1918
        "::5efe:a9fe:a9fe", // 169.254.169.254 — cloud metadata
    ] {
        let addr = ip(literal);
        assert!(is_forbidden_ip(&addr), "{literal}: flat path");
        for flag in [false, true] {
            assert!(
                is_forbidden_ip_with_policy(&addr, flag, &blanket),
                "{literal}: policy path (allow_loopback={flag})"
            );
            assert!(
                is_forbidden_resolved_address(&addr, "probe.lan", flag, &blanket),
                "{literal}: resolved path (allow_loopback={flag})"
            );
        }
    }
}

/// #2232 (ISATAP) positive control: an ISATAP of a PUBLIC address stays
/// ALLOWED — the guard re-validates the EMBEDDED IPv4, it does not refuse
/// the encapsulation form as a whole.
#[test]
fn isatap_embedded_public_address_stays_allowed() {
    let empty = hand_built(&[], &[]);
    for literal in ["::5efe:8.8.8.8", "::5efe:808:808"] {
        let addr = ip(literal);
        assert!(!is_forbidden_ip(&addr), "{literal}: flat path");
        assert!(!is_always_denied_ip(&addr), "{literal}: dial-layer set");
        for flag in [false, true] {
            assert!(
                !is_forbidden_ip_with_policy(&addr, flag, &empty),
                "{literal}: policy path (allow_loopback={flag})"
            );
        }
    }
}

/// #2232 (ISATAP) allowlist opt-in: the explicit allowlist must still reach
/// a legitimate private address THROUGH the encapsulation, exactly as it
/// reaches the plain address. The encapsulation is not a blanket: the
/// embedded IPv4 is what the policy judges.
#[test]
fn isatap_allowlist_opt_in_still_reaches_the_embedded_address() {
    let lan = hand_built(&["10.0.0.0/8"], &["vllm.lan"]);
    let addr = ip("::5efe:a00:1");
    for flag in [false, true] {
        assert!(
            !is_forbidden_ip_with_policy(&addr, flag, &lan),
            "policy path (allow_loopback={flag})"
        );
        assert!(
            !is_forbidden_resolved_address(&addr, "vllm.lan", flag, &lan),
            "resolved path (allow_loopback={flag})"
        );
    }
    // The plain address keeps the very same verdict.
    let plain = ip("10.0.0.1");
    assert!(!is_forbidden_ip_with_policy(&plain, false, &lan));
    assert!(!is_forbidden_resolved_address(
        &plain, "vllm.lan", false, &lan
    ));
}

/// #2231 (IPv4-compatible form): `::2` is the obsolete spelling of 0.0.0.2.
/// The flat path denies it (it re-validates the compatible form as IPv4 via
/// `to_ipv4()`), but the always-denied set normalized ONLY the IPv4-MAPPED
/// spelling — so the policy path ALLOWED it under an allowlist covering
/// the compatible space. The three verdict paths must agree.
#[test]
fn ipv4_compatible_form_is_denied_on_every_path() {
    let compatible = hand_built(&["::/120"], &["probe.lan"]);
    let addr = ip("::2");
    assert!(is_forbidden_ip(&addr), "flat path");
    for flag in [false, true] {
        assert!(
            is_forbidden_ip_with_policy(&addr, flag, &compatible),
            "policy path (allow_loopback={flag})"
        );
        assert!(
            is_forbidden_resolved_address(&addr, "probe.lan", flag, &compatible),
            "resolved path (allow_loopback={flag})"
        );
    }
    // Without the host entry the resolved path denies it through the host
    // gate — before and after the fix alike.
    let cidr_only = hand_built(&["::/120"], &[]);
    assert!(is_forbidden_resolved_address(
        &addr,
        "probe.lan",
        false,
        &cidr_only
    ));
}

/// #2231 (IPv4-compatible form) controls: the allowlist opt-in must keep
/// working for the plain and the mapped spellings, and a public address in
/// compatible form must stay allowed.
#[test]
fn ipv4_compatible_and_mapped_forms_keep_their_verdicts() {
    let lan = hand_built(&["10.0.0.0/8"], &["vllm.lan"]);
    for literal in ["10.0.0.1", "::ffff:10.0.0.1"] {
        let addr = ip(literal);
        assert!(
            !is_forbidden_ip_with_policy(&addr, false, &lan),
            "{literal}: policy path"
        );
        assert!(
            !is_forbidden_resolved_address(&addr, "vllm.lan", false, &lan),
            "{literal}: resolved path"
        );
    }
    let public = ip("::8.8.8.8");
    assert!(!is_forbidden_ip(&public), "flat path");
    assert!(!is_always_denied_ip(&public), "dial-layer set");
    assert!(
        !is_forbidden_ip_with_policy(&public, false, &lan),
        "policy path"
    );
}

/// The config-validation layer must refuse the ranges the dial layer now
/// denies, and must keep accepting the legitimate ones.
#[test]
fn config_validation_refuses_the_new_ranges_and_accepts_the_rest() {
    let rejected = [
        ("0.0.0.0/8", "this-network"),
        ("0.1.0.0/16", "this-network"),
        ("64:ff9b:1::/64", "NAT64"),
        ("64:ff9b:1::/48", "NAT64"),
    ];
    for (cidr, class) in rejected {
        let json =
            format!("{{\"mode\":\"allowlist\",\"allow_cidrs\":[\"{cidr}\"],\"allow_hosts\":[]}}");
        let err = serde_json::from_str::<NetworkPolicy>(&json)
            .expect_err("the never-allowable range must be refused");
        assert!(
            err.to_string().contains(class),
            "{cidr}: error must name the class, got: {err}"
        );
    }
    for cidr in ["10.0.0.0/8", "8.8.8.8/32", "2001:db8::/32"] {
        let json =
            format!("{{\"mode\":\"allowlist\",\"allow_cidrs\":[\"{cidr}\"],\"allow_hosts\":[]}}");
        serde_json::from_str::<NetworkPolicy>(&json)
            .unwrap_or_else(|e| panic!("{cidr} must stay allowlistable, got: {e}"));
    }
}
