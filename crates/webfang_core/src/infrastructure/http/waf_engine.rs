//! WAF Detection Engine - Layer 7 Protection
//!
//! This module provides advanced WAF detection beyond the basic signature matching
//! in http_client.rs. It includes:
//! - Detection by Control Headers (x-datadome-response, cf-mitigated, etc.)
//! - Entropy analysis for "Silent Challenge" detection
//! - Efficient O(N) matching using Aho-Corasick for 60+ signatures
//! - Body-only detection via `detect_body()` for callers that only have the body
//!
//! # Usage
//!
//! ```rust
//! use webfang::infrastructure::http::waf_engine::WafInspector;
//!
//! // Full integrity check (headers + body)
//! if let Err(e) = WafInspector::verify_integrity(&response, &body) {
//!     return Err(e);
//! }
//!
//! // Body-only check (replaces legacy detect_waf_challenge)
//! if let Some(provider) = WafInspector::detect_body(&body) {
//!     eprintln!("WAF detected: {provider}");
//! }
//! ```

use crate::error::ScraperError;
use aho_corasick::AhoCorasick;
use once_cell::sync::Lazy;
use std::collections::HashSet;
use wreq::header::HeaderMap;

/// Control headers that indicate WAF processing (2026 signatures)
const WAF_CONTROL_HEADERS: &[(&str, &str)] = &[
    ("x-datadome-response", "DataDome"),
    ("cf-mitigated", "Cloudflare"),
    ("x-akamai-edge-auth", "Akamai"),
    ("x-sucuri-id", "Sucuri"),
    ("x-wordpress", "Wordfence"),
    ("cf-ray", "Cloudflare"),
    ("x-cdn", "Imperva"),
];

/// Merged WAF signatures for body scanning (62 unique patterns).
/// Combined from legacy `waf.rs` (41) + `waf_engine.rs` (39), deduplicated by pattern string.
const WAF_BODY_SIGNATURES: &[(&str, &str)] = &[
    // Cloudflare
    ("cf-turnstile", "Cloudflare Turnstile"),
    ("challenge-platform", "Cloudflare JS Challenge"),
    ("Just a moment...", "Cloudflare"),
    ("Checking your browser", "Cloudflare"),
    ("__cf_chl_f_tk", "Cloudflare"),
    ("cf-browser-verification", "Cloudflare"),
    ("cf-ray", "Cloudflare"),
    ("cf-cache-status", "Cloudflare"),
    ("_cf_chl_opt", "Cloudflare"),
    ("cloudflare", "Cloudflare"),
    ("cf-dns", "Cloudflare"),
    // Google reCAPTCHA
    ("g-recaptcha", "reCAPTCHA"),
    ("recaptcha/api.js", "reCAPTCHA"),
    ("grecaptcha.execute", "reCAPTCHA"),
    ("recaptcha.net", "reCAPTCHA"),
    ("recaptcha Enterprise", "reCAPTCHA"),
    // hCaptcha
    ("hcaptcha.com", "hCaptcha"),
    ("h-captcha", "hCaptcha"),
    ("hcaptcha-api", "hCaptcha"),
    ("hcaptcha.js", "hCaptcha"),
    // DataDome
    ("datadome", "DataDome"),
    ("dd-captcha", "DataDome"),
    ("datadome.co", "DataDome"),
    ("dd=", "DataDome"),
    ("data-domain", "DataDome"),
    // PerimeterX / HUMAN Security
    ("perimeterx", "PerimeterX"),
    ("_pxCaptcha", "PerimeterX"),
    ("px-captcha", "PerimeterX"),
    ("perimeterx.net", "PerimeterX"),
    ("human-security", "HUMAN"),
    ("px-init", "PerimeterX"),
    // Akamai Bot Manager
    ("_abck", "Akamai Bot Manager"),
    ("SensorData", "Akamai Bot Manager"),
    ("akamai-bot-manager", "Akamai Bot Manager"),
    ("akamai.net", "Akamai"),
    ("akamai", "Akamai"),
    // Imperva / Incapsula
    ("imperva", "Imperva"),
    ("incapsula", "Imperva"),
    ("_Incapsula_Resource", "Imperva"),
    ("visid_incap", "Imperva Incapsula"),
    ("incap_ses", "Imperva Incapsula"),
    // Sucuri
    ("sucuri", "Sucuri"),
    ("sucuri.net", "Sucuri"),
    // F5
    ("_nfv", "F5"),
    ("BIGipServer", "F5"),
    // Generic challenges
    ("Please verify you are a human", "Generic Challenge"),
    ("verify you are human", "Generic Challenge"),
    ("bot detection", "Generic Detection"),
    ("automated requests", "Generic Detection"),
    ("security check", "Generic Challenge"),
    ("anti-bot", "Generic Detection"),
    ("checking your browser", "Browser Verification"),
    ("attack detected", "Security Firewall"),
    ("suspicious activity", "Security Firewall"),
    ("captcha-delivery", "Challenge Delivery"),
    ("__js_challenge__", "JS Challenge"),
    // Generic JS/CAPTCHA scripts
    ("challenge.js", "Generic Challenge"),
    ("captcha.js", "Generic Challenge"),
    ("verify.js", "Generic Challenge"),
    ("bot-check", "Generic Detection"),
    // AWS WAF (Amazon Web Services WAF)
    ("awsWafCookieDomainList", "AWS WAF"),
    ("AwsWafIntegration", "AWS WAF"),
    ("gokuProps", "AWS WAF"),
    ("aws-waf-token", "AWS WAF"),
];

/// Shannon entropy threshold for obfuscated WAF detection
const ENTROPY_THRESHOLD: f64 = 5.5;

/// Body size threshold (100KB) above which entropy analysis is applied
const SUSPICIOUS_SIZE_THRESHOLD: usize = 100_000;

/// Aho-Corasick automaton for O(N) multi-pattern body matching
/// Replaces O(N*M) linear scan in legacy `waf.rs`.
/// Compiled once via `Lazy`, thread-safe for concurrent reads.
static WAF_AC: Lazy<AhoCorasick> = Lazy::new(|| {
    AhoCorasick::new(WAF_BODY_SIGNATURES.iter().map(|(sig, _)| sig))
        .expect("Failed to build Aho-Corasick automaton")
});

/// WafInspector provides multi-layer WAF detection
pub struct WafInspector;

impl WafInspector {
    /// Scan body for WAF challenge signatures using Aho-Corasick (O(N) single pass).
    ///
    /// Returns the FIRST matching provider name, or `None` if the body is clean.
    /// For bodies exceeding 100KB, Shannon entropy is computed; if entropy > 5.5,
    /// returns `Some("Obfuscated WAF")`.
    ///
    /// Thread-safe: the AC automaton is immutable once compiled via `Lazy`.
    ///
    /// # Arguments
    /// * `body` - The HTTP response body to scan
    ///
    /// # Returns
    /// * `Some(provider_name)` - WAF challenge detected
    /// * `None` - No WAF challenge detected
    #[must_use]
    pub fn detect_body(body: &str) -> Option<&'static str> {
        // Early exit for empty or very small bodies (no signatures fit in <10 chars)
        if body.len() < 10 {
            return None;
        }

        // Shannon entropy check for large bodies (>100KB)
        if body.len() > SUSPICIOUS_SIZE_THRESHOLD {
            let entropy = calculate_entropy(body);
            if entropy > ENTROPY_THRESHOLD {
                return Some("Obfuscated WAF");
            }
        }

        // Aho-Corasick single-pass scan for all 62 patterns.
        // Returns provider name for the first match found by AC (earliest end position).
        WAF_AC
            .find(body)
            .map(|m| WAF_BODY_SIGNATURES[m.pattern()].1)
    }

    /// Verify response integrity across multiple layers
    ///
    /// 1. Control Headers: Check for WAF-specific headers (immediate)
    /// 2. Body Signatures: O(N) scan using Aho-Corasick
    /// 3. Entropy Analysis: Detect "Silent Challenges" in minimal HTML
    ///
    /// # Arguments
    /// * `headers` - Response headers from HTTP call
    /// * `body` - Response body (HTML content)
    ///
    /// # Returns
    /// * `Ok(())` - No WAF challenge detected
    /// * `Err(ScraperError::WafBlocked)` - WAF challenge detected
    pub fn verify_integrity(headers: &HeaderMap, body: &str) -> Result<(), ScraperError> {
        // Layer 1: Control Headers (fastest - O(1) lookup)
        Self::check_control_headers(headers)?;

        // Layer 2: Body Signature Matching (O(N) with Aho-Corasick)
        Self::check_body_signatures(body)?;

        // Layer 3: Entropy Analysis (detect Silent Challenges)
        Self::check_entropy(body)?;

        Ok(())
    }

    /// Check for WAF control headers that indicate bot detection/processing
    #[inline]
    fn check_control_headers(headers: &HeaderMap) -> Result<(), ScraperError> {
        for (header_name, provider) in WAF_CONTROL_HEADERS {
            // Check if header exists (even with empty value indicates WAF processing)
            if headers.get(*header_name).is_some() {
                // Some headers like cf-ray exist even for normal requests,
                // but others like x-datadome-response specifically indicate bot challenges
                if *header_name == "x-datadome-response"
                    || *header_name == "cf-mitigated"
                    || *header_name == "x-akamai-edge-auth"
                {
                    return Err(ScraperError::WafBlocked {
                        url: String::new(),
                        provider: format!("{provider}: header detected"),
                    });
                }
            }
        }
        Ok(())
    }

    /// Check body content for WAF signatures using O(N) Aho-Corasick
    #[inline]
    fn check_body_signatures(body: &str) -> Result<(), ScraperError> {
        // Early exit for empty or very small bodies
        // Lowered to 10 chars to detect short WAF challenge pages
        if body.len() < 10 {
            return Ok(());
        }

        // Use Aho-Corasick for O(N) multi-pattern matching
        if let Some(mat) = WAF_AC.find_iter(body).next() {
            // Map pattern index to provider name
            let provider = WAF_BODY_SIGNATURES[mat.pattern()].1;
            return Err(ScraperError::WafBlocked {
                url: String::new(),
                provider: format!("Signature detected: {provider}"),
            });
        }

        Ok(())
    }

    /// Detect "Silent Challenges" using entropy analysis
    ///
    /// WAFs in 2026 sometimes return HTTP 200 with minimal HTML containing
    /// heavy JavaScript challenges. This function detects that pattern:
    /// - Body < 1500 bytes
    /// - High density of <script> tags (> 5)
    /// - Low text content ratio
    #[inline]
    fn check_entropy(body: &str) -> Result<(), ScraperError> {
        // Only analyze bodies under 1500 bytes
        if body.len() > 1500 {
            return Ok(());
        }

        // Count <script> tags efficiently
        let script_count = body.matches("<script").count();

        // Silent Challenge detection:
        // - Multiple script tags in a small body suggests JS challenge
        // - Low text ratio indicates mostly code, not content
        if script_count > 5 && body.len() < 1000 {
            return Err(ScraperError::WafBlocked {
                url: String::new(),
                provider: "Silent Challenge: High JS density in minimal body".into(),
            });
        }

        // Additional entropy check: ratio of script to text
        if body.len() < 500 && script_count > 3 {
            return Err(ScraperError::WafBlocked {
                url: String::new(),
                provider: "Silent Challenge: Suspicious script/text ratio".into(),
            });
        }

        Ok(())
    }

    /// Get the list of supported WAF providers
    #[must_use]
    pub fn supported_providers() -> Vec<&'static str> {
        // Extract unique provider names from signatures
        let mut providers: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();

        for (_, provider) in WAF_BODY_SIGNATURES {
            if !seen.contains(provider) {
                seen.insert(provider);
                providers.push(provider);
            }
        }
        providers.sort();
        providers
    }
}

/// Calculate Shannon entropy of a string
///
/// Used to detect obfuscated JavaScript in challenge pages, which often have
/// high entropy due to minification and encoding.
///
/// # Arguments
/// * `s` - The string to analyze
///
/// # Returns
/// * Entropy value between 0.0 (uniform distribution) and ~8.0 (high entropy)
#[inline]
fn calculate_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }

    let mut freq = [0u32; 256];
    let len = s.len() as f64;

    for &b in s.as_bytes() {
        freq[b as usize] += 1;
    }

    let mut entropy = 0.0;
    for &count in &freq {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }

    entropy
}

/// Check if body size indicates a potential WAF challenge (>100KB)
#[cfg(test)]
#[inline]
fn is_suspicious_size(body_len: usize) -> bool {
    body_len > SUSPICIOUS_SIZE_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // detect_body() tests — ported from waf.rs (Approval Testing)
    // ========================================================================

    #[test]
    fn test_detect_body_cloudflare_turnstile() {
        let html = r#"<div id="cf-turnstile" data-sitekey="abc123"></div>"#;
        assert_eq!(
            WafInspector::detect_body(html),
            Some("Cloudflare Turnstile")
        );
    }

    #[test]
    fn test_detect_body_cloudflare_just_a_moment() {
        let html = "<html><body><h1>Just a moment...</h1></body></html>";
        assert_eq!(WafInspector::detect_body(html), Some("Cloudflare"));
    }

    #[test]
    fn test_detect_body_cloudflare_checking_browser() {
        let html = "<html><body>Checking your browser before accessing...</body></html>";
        assert_eq!(WafInspector::detect_body(html), Some("Cloudflare"));
    }

    #[test]
    fn test_detect_body_recaptcha() {
        let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=abc"></script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("reCAPTCHA"));
    }

    #[test]
    fn test_detect_body_g_recaptcha() {
        let html = r#"<div class="g-recaptcha" data-sitekey="abc"></div>"#;
        assert_eq!(WafInspector::detect_body(html), Some("reCAPTCHA"));
    }

    #[test]
    fn test_detect_body_hcaptcha() {
        let html = r#"<div class="h-captcha" data-sitekey="abc"></div>"#;
        assert_eq!(WafInspector::detect_body(html), Some("hCaptcha"));
    }

    #[test]
    fn test_detect_body_datadome() {
        let html = r#"<script src="https://js.datadome.co/captcha.js"></script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("DataDome"));
    }

    #[test]
    fn test_detect_body_perimeterx() {
        let html = r#"<script>var _pxCaptcha = {};</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("PerimeterX"));
    }

    #[test]
    fn test_detect_body_akamai() {
        let html = r#"<input type="hidden" name="_abck" value="xxx">"#;
        assert_eq!(WafInspector::detect_body(html), Some("Akamai Bot Manager"));
    }

    #[test]
    fn test_detect_body_generic_challenge() {
        let html = "<p>Please verify you are a human to continue.</p>";
        assert_eq!(WafInspector::detect_body(html), Some("Generic Challenge"));
    }

    #[test]
    fn test_detect_body_clean_html() {
        let html = r#"
            <html>
                <head><title>Normal Page</title></head>
                <body>
                    <article>
                        <h1>Welcome</h1>
                        <p>This is a normal page with real content.</p>
                    </article>
                </body>
            </html>
        "#;
        assert_eq!(WafInspector::detect_body(html), None);
    }

    #[test]
    fn test_detect_body_empty() {
        assert_eq!(WafInspector::detect_body(""), None);
    }

    #[test]
    fn test_detect_body_aws_waf_cookie_domain_list() {
        let html = r#"<script>window.awsWafCookieDomainList = [];</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    #[test]
    fn test_detect_body_aws_waf_integration() {
        let html = r#"<script>AwsWafIntegration.saveReferrer();</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    #[test]
    fn test_detect_body_aws_waf_goku_props() {
        let html = r#"<script>window.gokuProps = {"key":"AQIDAH..."};</script>"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    #[test]
    fn test_detect_body_aws_waf_token() {
        let html = r#"<meta name="aws-waf-token" content="abc123">"#;
        assert_eq!(WafInspector::detect_body(html), Some("AWS WAF"));
    }

    // ========================================================================
    // Entropy tests — ported from waf.rs
    // ========================================================================

    #[test]
    fn test_calculate_entropy_high() {
        let obfuscated_js: String = (0u8..=255).map(|b| b as char).collect();
        let entropy = calculate_entropy(&obfuscated_js);
        assert!(entropy > 6.0, "entropy={entropy}, expected > 6.0");
    }

    #[test]
    fn test_calculate_entropy_low() {
        let plain_text = "Hello world, this is a normal page with regular content.";
        let entropy = calculate_entropy(plain_text);
        assert!(entropy < 5.0);
    }

    #[test]
    fn test_is_suspicious_size() {
        assert!(is_suspicious_size(150_000));
        assert!(!is_suspicious_size(10_000));
        assert!(!is_suspicious_size(100_000));
        assert!(is_suspicious_size(100_001));
    }

    #[test]
    fn test_detect_body_by_entropy() {
        // Create >100KB with high entropy to trigger Shannon entropy detection
        let high_entropy_content: String = (0u8..=255)
            .map(|b| b as char)
            .chain((0u8..=255).map(|b| b as char))
            .chain((0u8..=255).map(|b| b as char))
            .chain((0u8..=255).map(|b| b as char))
            .cycle()
            .take(104_000)
            .collect();
        let result = WafInspector::detect_body(&high_entropy_content);
        assert_eq!(result, Some("Obfuscated WAF"));
    }

    #[test]
    fn test_detect_body_small_low_entropy() {
        let small_content = "<html><body>Redirecting...</body></html>";
        assert_eq!(WafInspector::detect_body(small_content), None);
    }

    // ========================================================================
    // verify_integrity() tests (existing, unchanged)
    // ========================================================================

    #[test]
    fn test_waf_control_header_detection() {
        // Test DataDome header detection
        let mut headers = HeaderMap::new();
        headers.insert("x-datadome-response", "blocked".parse().unwrap());

        let result = WafInspector::verify_integrity(&headers, "normal content");
        assert!(result.is_err());

        // Test that cf-ray alone doesn't trigger (common in normal requests)
        let mut headers = HeaderMap::new();
        headers.insert("cf-ray", "abc123".parse().unwrap());

        let result = WafInspector::verify_integrity(&headers, "normal content");
        assert!(result.is_ok());
    }

    #[test]
    fn test_waf_body_signature_detection() {
        // Test Cloudflare detection
        let result = WafInspector::verify_integrity(&HeaderMap::new(), "Just a moment...");
        assert!(result.is_err());

        // Test reCAPTCHA detection
        let result = WafInspector::verify_integrity(&HeaderMap::new(), "<div class='g-recaptcha'>");
        assert!(result.is_err());

        // Test normal content passes
        let result = WafInspector::verify_integrity(
            &HeaderMap::new(),
            "<html><body><p>Hello World</p></body></html>",
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_silent_challenge_detection() {
        let body = r#"<html><script></script><script></script><script></script><script></script><script></script><script></script></html>"#;
        let result = WafInspector::verify_integrity(&HeaderMap::new(), body);
        assert!(result.is_err());

        let body = "<html><body><p>Hello</p></body></html>";
        let result = WafInspector::verify_integrity(&HeaderMap::new(), body);
        assert!(result.is_ok());
    }

    #[test]
    fn test_aho_corasick_performance() {
        let body = "This is a page with Just a moment... and recaptcha/api.js content";
        let result = WafInspector::verify_integrity(&HeaderMap::new(), body);
        assert!(result.is_err());
    }

    #[test]
    fn test_supported_providers() {
        let providers = WafInspector::supported_providers();
        assert!(!providers.is_empty());
        assert!(providers.contains(&"Cloudflare"));
        assert!(providers.contains(&"reCAPTCHA"));
        assert!(providers.contains(&"DataDome"));
        assert!(providers.contains(&"AWS WAF"));
    }
}
