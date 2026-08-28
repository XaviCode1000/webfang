//! Behavioral fixture tests for context-aware WAF detection (TASK-09).
//!
//! Acceptance for REQ-WAF-02/03/04/05: each fixture is served over real HTTP
//! (wiremock) with its correct status + content-type, fetched with `wreq`, and
//! inspected via the shared [`WafInspector::inspect`] with a full
//! [`InspectionContext`] built from the actual response. The verdict is
//! snapshot-tested (insta) for determinism.
//!
//! Fixture → verdict map (spec #1188):
//!
//! | # | Fixture                | Status | Content-Type     | Verdict |
//! |---|------------------------|--------|------------------|---------|
//! | 1 | json_akamai_hash.json  | 200    | application/json | PASS    |
//! | 2 | cloudflare_503.html    | 503    | text/html        | BLOCK   |
//! | 3 | turnstile_200.html     | 200    | text/html        | BLOCK   |
//! | 4 | news_cloudflare_200.html | 200  | text/html        | PASS    |
//! | 5 | incap_ses_200.html     | 200    | text/html        | PASS    |

#[path = "common/cli_harness.rs"]
mod common;

use common::{redact_nondeterministic, BehavioralTest};
use std::path::Path;
use webfang_core::domain::waf::{InspectionContext, WafInspector, WafVerdict};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Workspace-root-relative fixtures directory (`CARGO_MANIFEST_DIR` is
/// `crates/webfang_core`, two levels below the workspace root).
fn fixtures_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/waf")
}

/// Render a deterministic, human-readable verdict summary for snapshotting
/// (mirrors the Spanish evidence-chain format of REQ-WAF-08).
fn verdict_summary(verdict: &WafVerdict) -> String {
    let mut lines = vec![format!("is_blocked: {}", verdict.is_blocked)];
    for e in &verdict.evidences {
        lines.push(format!(
            "  - {} (patrón: {}, tier: {})",
            e.provider,
            e.matched_pattern,
            e.tier.label_es()
        ));
    }
    lines.join("\n")
}

/// Serve `body` at `route` with the given status + content-type, fetch it over
/// real HTTP, and inspect it with a full context built from the response.
async fn inspect_served_fixture(
    route: &str,
    body: String,
    status: u16,
    content_type: &str,
    ignore_waf: bool,
) -> WafVerdict {
    let harness = BehavioralTest::new().await;
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_string(body)
                .insert_header("Content-Type", content_type),
        )
        .mount(&harness.server)
        .await;

    let url = format!("{}{}", harness.server.uri(), route);
    let client = wreq::Client::new();
    let response = client
        .get(&url)
        .send()
        .await
        .expect("fetch fixture over HTTP");
    let mut header_map = std::collections::HashMap::new();
    for (k, v) in response.headers().iter() {
        if let Ok(val) = v.to_str() {
            header_map.insert(k.as_str().to_lowercase(), val.to_string());
        }
    }
    let ctx = InspectionContext {
        status: Some(response.status().as_u16()),
        content_type: response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        headers: header_map,
        ignore_waf,
    };
    let body = response.text().await.expect("read response body");
    WafInspector::inspect(&body, &ctx)
}

/// Load a fixture file, serve + inspect it, and snapshot the verdict.
async fn snapshot_fixture(name: &str, route: &str, status: u16, content_type: &str) {
    let body = std::fs::read_to_string(fixtures_dir().join(name))
        .unwrap_or_else(|e| panic!("read fixture {name}: {e}"));
    let verdict = inspect_served_fixture(route, body, status, content_type, false).await;
    let summary = verdict_summary(&verdict);
    let snap_name = name.trim_end_matches(".json").trim_end_matches(".html");
    insta::assert_snapshot!(
        snap_name,
        redact_nondeterministic(Path::new("__no_temp__"), &summary)
    );
}

#[tokio::test]
async fn fixture_1_json_akamai_hash_pass() {
    // REQ-02 (json skip) + REQ-04 ([B] rejects '_') + REQ-05 (T2@200): PASS.
    snapshot_fixture(
        "json_akamai_hash.json",
        "/api/fingerprint",
        200,
        "application/json",
    )
    .await;
}

#[tokio::test]
async fn fixture_2_cloudflare_503_block() {
    // REQ-03 (T1 prose) + REQ-05 (T1 any status + 503): BLOCK.
    snapshot_fixture("cloudflare_503.html", "/challenge", 503, "text/html").await;
}

#[tokio::test]
async fn fixture_3_turnstile_200_block() {
    // REQ-03 (cf-turnstile T1) + REQ-05 (T1@200): BLOCK.
    snapshot_fixture("turnstile_200.html", "/login", 200, "text/html").await;
}

#[tokio::test]
async fn fixture_4_news_cloudflare_200_pass() {
    // REQ-04 ([B] stands in prose) + REQ-05 (T2@200): PASS.
    snapshot_fixture("news_cloudflare_200.html", "/article", 200, "text/html").await;
}

#[tokio::test]
async fn fixture_5_incap_ses_200_pass() {
    // REQ-03 ([E] exempt) + REQ-05 (T2@200): PASS.
    snapshot_fixture("incap_ses_200.html", "/dashboard", 200, "text/html").await;
}

/// REQ-WAF-07: `--ignore-waf` (ctx.ignore_waf) short-circuits to a clean verdict
/// on EVERY fixture — including the two that block by default (cloudflare_503,
/// turnstile_200). One snapshot captures all five verdicts as not-blocked.
#[tokio::test]
async fn ignore_waf_yields_clean_verdict_on_all_fixtures() {
    let fixtures = [
        (
            "json_akamai_hash.json",
            "/api/fingerprint",
            200u16,
            "application/json",
        ),
        ("cloudflare_503.html", "/challenge", 503, "text/html"),
        ("turnstile_200.html", "/login", 200, "text/html"),
        ("news_cloudflare_200.html", "/article", 200, "text/html"),
        ("incap_ses_200.html", "/dashboard", 200, "text/html"),
    ];
    let mut lines = Vec::new();
    for (name, route, status, ct) in fixtures {
        let body = std::fs::read_to_string(fixtures_dir().join(name))
            .unwrap_or_else(|e| panic!("read fixture {name}: {e}"));
        let verdict = inspect_served_fixture(route, body, status, ct, true).await;
        assert!(
            !verdict.is_blocked,
            "{name} must be clean under ignore_waf, got {verdict:?}"
        );
        assert!(
            verdict.evidences.is_empty(),
            "{name} must carry no evidence under ignore_waf (short-circuit)"
        );
        lines.push(format!("{name}: is_blocked={}", verdict.is_blocked));
    }
    insta::assert_snapshot!(
        "ignore_waf_all_fixtures",
        redact_nondeterministic(Path::new("__no_temp__"), &lines.join("\n"))
    );
}
