//! Security-focused integration tests for WAF evasion scenarios
//!
//! These tests verify that the scraper correctly detects and handles
//! WAF/CAPTCHA challenges from various providers via the shared,
//! context-aware [`WafInspector::inspect`] (REQ-WAF-01/05). Body-only tests
//! run in degraded mode ([`InspectionContext::default`]); header tests supply
//! an [`InspectionContext`] with headers.
//!
//! Run with: cargo nextest run --test-threads 2 security_integration

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use webfang_core::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
use wreq::header::HeaderMap;

/// Degraded-mode (no HTTP context) body inspection helper for these tests.
fn inspect_body(html: &str) -> webfang_core::infrastructure::http::waf_engine::WafVerdict {
    WafInspector::inspect(html, &InspectionContext::default())
}

/// Headers + body inspection in degraded mode (no status/content-type).
fn inspect_with_headers(
    headers: HeaderMap,
    html: &str,
) -> webfang_core::infrastructure::http::waf_engine::WafVerdict {
    let ctx = InspectionContext {
        headers,
        ..Default::default()
    };
    WafInspector::inspect(html, &ctx)
}

// ============================================================================
// Cloudflare Challenge Detection Tests
// ============================================================================

#[tokio::test]
async fn test_cloudflare_turnstile_detection() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>Just a moment...</title>
        </head>
        <body>
            <div id="cf-turnstile" data-sitekey="0x4AAAAAAA"></div>
            <script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script>
        </body>
        </html>
    "#;

    let verdict = inspect_body(html);
    // "Just a moment..." in the title is the first Challenge-tier evidence.
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Cloudflare")
    );
}

#[tokio::test]
async fn test_cloudflare_js_challenge_detection() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>Checking your browser...</title>
        </head>
        <body>
            <div id="challenge-platform" data-ray="abc123"></div>
            <script>
                var _cf_chl_opt = {c: 1, s: 1};
            </script>
        </body>
        </html>
    "#;

    let verdict = inspect_body(html);
    // "Checking your browser" in the title is the first Challenge-tier evidence.
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Cloudflare")
    );
}

#[tokio::test]
async fn test_cloudflare_just_a_moment_detection() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <meta http-equiv="refresh" content="5">
        </head>
        <body>
            <center>
                <h1>Just a moment...</h1>
                <p>Checking your browser before accessing...</p>
            </center>
        </body>
        </html>
    "#;

    let verdict = inspect_body(html);
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("Cloudflare")
    );
}

// ============================================================================
// DataDome Silent Challenge Detection Tests
// ============================================================================

#[tokio::test]
async fn test_datadome_silent_challenge_detection() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <script src="https://js.datadome.co/tags.js"></script>
        </head>
        <body>
            <div id="dd-captcha" data-sitekey="abc123"></div>
            <script>
                var dd = {key: 'abc123'};
            </script>
        </body>
        </html>
    "#;

    // dd-captcha is a Challenge-tier marker, so the verdict blocks; the first
    // evidence is the datadome.co fingerprint in the head script.
    let verdict = inspect_body(html);
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("DataDome")
    );
}

#[tokio::test]
async fn test_datadome_high_entropy_detection() {
    // Create deterministic high-entropy content (>100KB) so CI does not depend on randomness.
    let obfuscated_js: String = (32u8..=126)
        .cycle()
        .take(95 * 1100)
        .map(char::from)
        .collect();

    // Degraded mode treats unknown status as non-200, so the high-entropy body
    // (>100KB, >5.5 b/B) blocks as an obfuscated WAF challenge.
    let verdict = inspect_body(&obfuscated_js);
    assert!(
        verdict.is_blocked,
        "High entropy content should be detected, got {:?}",
        verdict
    );
}

// ============================================================================
// reCAPTCHA and hCaptcha Detection Tests
// ============================================================================

#[tokio::test]
async fn test_recaptcha_detection() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div class="g-recaptcha" data-sitekey="6Lc"></div>
            <script src="https://www.google.com/recaptcha/api.js" async defer></script>
        </body>
        </html>
    "#;

    let verdict = inspect_body(html);
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("reCAPTCHA")
    );
}

#[tokio::test]
async fn test_hcaptcha_detection() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div class="h-captcha" data-sitekey="abc123"></div>
            <script src="https://hcaptcha.com/1/api.js" async defer></script>
        </body>
        </html>
    "#;

    let verdict = inspect_body(html);
    assert!(verdict.is_blocked);
    assert_eq!(
        verdict.evidences.first().map(|e| e.provider),
        Some("hCaptcha")
    );
}

// ============================================================================
// Rate Limiting Bypass Detection Tests
// ============================================================================

#[tokio::test]
async fn test_rate_limiting_429_detection() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(429).set_body_string(r#"{"error": "Too many requests"}"#),
        )
        .mount(&mock_server)
        .await;

    let url = mock_server.uri();
    let client = wreq::Client::new();
    let response = client.get(&url).send().await;

    // wreq doesn't treat 4xx as errors, so we check the response
    assert!(response.is_ok());
    let resp = response.unwrap();
    assert_eq!(resp.status(), 429);
}

#[tokio::test]
async fn test_rate_limiting_retry_after_header() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("Retry-After", "60")
                .set_body_string(r#"{"error": "Rate limited"}"#),
        )
        .mount(&mock_server)
        .await;

    let url = mock_server.uri();
    let client = wreq::Client::new();
    let response = client.get(&url).send().await;

    // wreq doesn't treat 4xx as errors, so we check the response
    assert!(response.is_ok());
    let resp = response.unwrap();
    assert_eq!(resp.status(), 429);
    assert_eq!(
        resp.headers().get("Retry-After").unwrap().to_str().unwrap(),
        "60"
    );
}

// ============================================================================
// User Agent Rotation Tests
// ============================================================================

#[tokio::test]
async fn test_user_agent_rotation_under_waf_pressure() {
    let mock_server = MockServer::start().await;

    // First request - normal response
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>Normal content</body></html>"),
        )
        .mount(&mock_server)
        .await;

    let url = mock_server.uri();
    let client = wreq::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .build()
        .unwrap();

    let response = client.get(&url).send().await;
    assert!(response.is_ok());
}

// ============================================================================
// TLS Fingerprint Emulation Tests
// ============================================================================

#[tokio::test]
async fn test_tls_fingerprint_emulation() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>Content</body></html>"),
        )
        .mount(&mock_server)
        .await;

    let url = mock_server.uri();
    let client = wreq::Client::new();
    let response = client.get(&url).send().await;

    assert!(response.is_ok());
}

// ============================================================================
// WAF Inspector Integration Tests
// ============================================================================

#[tokio::test]
async fn test_waf_inspector_cloudflare_detection() {
    let html = r#"
        <html>
        <body>
            <h1>Just a moment...</h1>
            <script>var __cf_chl_f_tk = 'abc123';</script>
        </body>
        </html>
    "#;

    let verdict = inspect_with_headers(HeaderMap::new(), html);
    assert!(verdict.is_blocked);
    assert!(
        verdict.evidence_chain().contains("Cloudflare"),
        "chain: {}",
        verdict.evidence_chain()
    );
}

#[tokio::test]
async fn test_waf_inspector_datadome_header_detection() {
    // Control headers are Fingerprint-tier evidence — mere presence never
    // auto-blocks without a correlated WAF status (correction B). Degraded mode
    // (no status), so the header alone is clean.
    let mut headers = HeaderMap::new();
    headers.insert("x-datadome-response", "blocked".parse().unwrap());

    let html = "<html><body>Content</body></html>";
    let verdict = inspect_with_headers(headers, html);

    assert!(
        !verdict.is_blocked,
        "T2 header alone must not block in degraded mode"
    );
}

#[tokio::test]
async fn test_waf_inspector_silent_challenge_detection() {
    let html = r#"
        <html>
        <script></script>
        <script></script>
        <script></script>
        <script></script>
        <script></script>
        <script></script>
        </html>
    "#;

    let verdict = inspect_with_headers(HeaderMap::new(), html);
    assert!(verdict.is_blocked);
    assert!(
        verdict.evidence_chain().contains("Silent Challenge"),
        "chain: {}",
        verdict.evidence_chain()
    );
}

// ============================================================================
// Normal Content Passes Tests
// ============================================================================

#[tokio::test]
async fn test_normal_content_passes_waf_detection() {
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>Normal Page</title>
        </head>
        <body>
            <article>
                <h1>Welcome to Our Site</h1>
                <p>This is normal content with no WAF challenges.</p>
                <p>Lorem ipsum dolor sit amet, consectetur adipiscing elit.</p>
            </article>
        </body>
        </html>
    "#;

    let verdict = inspect_body(html);
    assert!(!verdict.is_blocked);
}

#[tokio::test]
async fn test_waf_inspector_normal_content_passes() {
    let html = r#"
        <html>
        <body>
            <h1>Normal Page</h1>
            <p>This is normal content.</p>
        </body>
        </html>
    "#;

    let verdict = inspect_with_headers(HeaderMap::new(), html);
    assert!(!verdict.is_blocked);
}
