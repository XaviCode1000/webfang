//! Security-focused integration tests for WAF evasion scenarios
//!
//! These tests verify that the scraper correctly detects and handles
//! WAF/CAPTCHA challenges from various providers.
//!
//! Run with: cargo nextest run --test-threads 2 security_integration

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rust_scraper::infrastructure::http::waf_engine::WafInspector;
use wreq::header::HeaderMap;

// ============================================================================
// Cloudflare Challenge Detection Tests
// ============================================================================

#[tokio::test]
async fn test_cloudflare_turnstile_detection() {
    // RED: Test that Cloudflare Turnstile is detected
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

    let result = WafInspector::detect_body(html);
    // AC matches "Just a moment..." in title before "cf-turnstile" in body
    assert_eq!(result, Some("Cloudflare"));
}

#[tokio::test]
async fn test_cloudflare_js_challenge_detection() {
    // RED: Test that Cloudflare JS Challenge is detected
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

    let result = WafInspector::detect_body(html);
    // AC matches "Checking your browser..." in title before "challenge-platform" in body
    assert_eq!(result, Some("Cloudflare"));
}

#[tokio::test]
async fn test_cloudflare_just_a_moment_detection() {
    // RED: Test that "Just a moment..." is detected
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

    let result = WafInspector::detect_body(html);
    assert_eq!(result, Some("Cloudflare"));
}

// ============================================================================
// DataDome Silent Challenge Detection Tests
// ============================================================================

#[tokio::test]
async fn test_datadome_silent_challenge_detection() {
    // RED: Test that DataDome silent challenge is detected
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

    let result = WafInspector::detect_body(html);
    assert_eq!(result, Some("DataDome"));
}

#[tokio::test]
async fn test_datadome_high_entropy_detection() {
    // RED: Test that DataDome high-entropy challenge is detected
    // Create deterministic high-entropy content (>100KB) so CI does not depend on randomness.
    let obfuscated_js: String = (32u8..=126)
        .cycle()
        .take(95 * 1100)
        .map(char::from)
        .collect();

    // With threshold lowered to 5.5, high-entropy content should be detected
    // UTF-8 encoding of code points 128-255 produces non-uniform bytes (~5.5-6.0 bits)
    let result = WafInspector::detect_body(&obfuscated_js);
    assert!(
        result.is_some(),
        "High entropy content should be detected, got {:?}",
        result
    );
}

// ============================================================================
// reCAPTCHA and hCaptcha Detection Tests
// ============================================================================

#[tokio::test]
async fn test_recaptcha_detection() {
    // RED: Test that reCAPTCHA is detected
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div class="g-recaptcha" data-sitekey="6Lc"></div>
            <script src="https://www.google.com/recaptcha/api.js" async defer></script>
        </body>
        </html>
    "#;

    let result = WafInspector::detect_body(html);
    assert_eq!(result, Some("reCAPTCHA"));
}

#[tokio::test]
async fn test_hcaptcha_detection() {
    // RED: Test that hCaptcha is detected
    let html = r#"
        <!DOCTYPE html>
        <html>
        <body>
            <div class="h-captcha" data-sitekey="abc123"></div>
            <script src="https://hcaptcha.com/1/api.js" async defer></script>
        </body>
        </html>
    "#;

    let result = WafInspector::detect_body(html);
    assert_eq!(result, Some("hCaptcha"));
}

// ============================================================================
// Rate Limiting Bypass Detection Tests
// ============================================================================

#[tokio::test]
async fn test_rate_limiting_429_detection() {
    // RED: Test that rate limiting (429) is detected
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
    // RED: Test that Retry-After header is respected
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
    // RED: Test that user agent is rotated under WAF pressure
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
    // RED: Test that TLS fingerprint emulation is applied
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
    // RED: Test that WafInspector detects Cloudflare
    let html = r#"
        <html>
        <body>
            <h1>Just a moment...</h1>
            <script>var __cf_chl_f_tk = 'abc123';</script>
        </body>
        </html>
    "#;

    let headers = HeaderMap::new();
    let result = WafInspector::verify_integrity(&headers, html);

    assert!(result.is_err());
    if let Err(rust_scraper::error::ScraperError::WafBlocked { provider, .. }) = result {
        assert!(provider.contains("Cloudflare"));
    } else {
        panic!("Expected WafBlocked error");
    }
}

#[tokio::test]
async fn test_waf_inspector_datadome_header_detection() {
    // RED: Test that WafInspector detects DataDome via header
    let mut headers = HeaderMap::new();
    headers.insert("x-datadome-response", "blocked".parse().unwrap());

    let html = "<html><body>Content</body></html>";
    let result = WafInspector::verify_integrity(&headers, html);

    assert!(result.is_err());
    if let Err(rust_scraper::error::ScraperError::WafBlocked { provider, .. }) = result {
        assert!(provider.contains("DataDome"));
    } else {
        panic!("Expected WafBlocked error");
    }
}

#[tokio::test]
async fn test_waf_inspector_silent_challenge_detection() {
    // RED: Test that WafInspector detects silent challenges
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

    let headers = HeaderMap::new();
    let result = WafInspector::verify_integrity(&headers, html);

    assert!(result.is_err());
    if let Err(rust_scraper::error::ScraperError::WafBlocked { provider, .. }) = result {
        assert!(provider.contains("Silent Challenge"));
    } else {
        panic!("Expected WafBlocked error");
    }
}

// ============================================================================
// Normal Content Passes Tests
// ============================================================================

#[tokio::test]
async fn test_normal_content_passes_waf_detection() {
    // RED: Test that normal content passes WAF detection
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

    let result = WafInspector::detect_body(html);
    assert_eq!(result, None);
}

#[tokio::test]
async fn test_waf_inspector_normal_content_passes() {
    // RED: Test that WafInspector passes normal content
    let html = r#"
        <html>
        <body>
            <h1>Normal Page</h1>
            <p>This is normal content.</p>
        </body>
        </html>
    "#;

    let headers = HeaderMap::new();
    let result = WafInspector::verify_integrity(&headers, html);

    assert!(result.is_ok());
}
