//! #1615, DF-E9 — `WEBFANG_DISABLE_SSRF` must not disarm the LLM base-URL
//! SSRF gate in a production build.
//!
//! ## Why this is an integration test and not a unit test
//!
//! The finding was a presence-based environment read on a PRODUCTION path:
//! `ssrf_gate` returned `Ok` for every base URL whenever the variable existed,
//! regardless of its value, so anything that could set one name in the
//! process environment had a kill-switch for an SSRF gate.
//!
//! The fix compiles the read out of production with `#[cfg(test)]`. A unit
//! test in `webfang_core` CANNOT observe that fix, because a unit test is
//! compiled with `cfg(test)` set for the crate under test — it sees the hatch.
//! Writing the regression row there would have produced a test that passes
//! before and after the fix, which is worse than no test.
//!
//! This file is a separate crate that links `webfang_core` as an external
//! dependency. `cfg(test)` is definitively off for the library, so the hatch
//! does not exist here, and setting the variable proves the gate ignores it.

#![allow(clippy::disallowed_methods)] // via EnvGuard, the sanctioned owner of env mutation

use webfang_core::application::llm_extraction::ssrf_gate;
use webfang_core::domain::ssrf_guard::LLM_SSRF_TEST_HATCH_ENV;

/// The regression row: a forbidden target is still refused with the variable
/// set, in every value the old presence-based read accepted.
///
/// `0`, `false` and the empty string are the interesting ones — a reader
/// would reasonably guess those mean "do not disable", and under the old code
/// each of them disabled the gate exactly like `1` did.
#[test]
fn the_llm_ssrf_hatch_does_not_disarm_the_gate_outside_a_test_build() {
    for value in ["1", "0", "false", "", "yes", "true", "please"] {
        let _guard = webfang_test_utils::EnvGuard::with(&[(LLM_SSRF_TEST_HATCH_ENV, value)]);
        for host in [
            "127.0.0.1",
            "169.254.169.254",
            "10.0.0.1",
            "192.168.1.1",
            "100.64.1.1",
            "[fc00::1]",
            "[::1]",
        ] {
            let url: url::Url = format!("http://{host}/v1").parse().expect("literal parses");
            assert!(
                ssrf_gate(&url).is_err(),
                "host {host} must stay refused with {LLM_SSRF_TEST_HATCH_ENV}={value:?} \
                 set — the gate is not disarmed by the environment outside a \
                 test build (#1615 DF-E9)"
            );
        }
    }
}

/// The scheme allow-list is part of the same gate and must be equally
/// unconditional: a hatch that only covered the IP check would still be a
/// production kill-switch for the scheme half.
#[test]
fn the_llm_ssrf_hatch_does_not_reopen_non_http_schemes() {
    for value in ["1", "0", "true"] {
        let _guard = webfang_test_utils::EnvGuard::with(&[(LLM_SSRF_TEST_HATCH_ENV, value)]);
        for url in [
            "ftp://8.8.8.8/v1",
            "file:///etc/passwd",
            "gopher://8.8.8.8/",
        ] {
            let parsed: url::Url = url.parse().expect("url parses");
            assert!(
                ssrf_gate(&parsed).is_err(),
                "{url} must stay refused with the hatch set to {value:?}"
            );
        }
    }
}

/// And the gate still admits what it should — a test that only asserted
/// refusals would pass if the gate simply rejected everything, which is a
/// different bug.
#[test]
fn a_public_llm_base_url_is_still_admitted_with_the_hatch_set() {
    let _guard = webfang_test_utils::EnvGuard::with(&[(LLM_SSRF_TEST_HATCH_ENV, "1")]);
    let url: url::Url = "https://api.example.com/v1"
        .parse()
        .expect("public url parses");
    assert!(
        ssrf_gate(&url).is_ok(),
        "the fix must not turn the gate into a blanket refusal"
    );
}
