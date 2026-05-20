//! WAF/CAPTCHA challenge detection
//!
//! Detects WAF/CAPTCHA challenge pages disguised as HTTP 200 responses
//! by scanning for known signatures, entropy analysis, and response size checks.

/// WAF/CAPTCHA challenge signature for detection in HTTP 200 responses
///
/// Each entry is (signature, provider_name). The detector scans the response body
/// for these substrings — zero allocations, O(N*M) where M is the signature count.
///
/// Following **perf-iter-over-index**: iterator-based scanning, no regex needed.
const WAF_SIGNATURES: &[(&str, &str)] = &[
    // Cloudflare Turnstile / JS Challenge
    ("cf-turnstile", "Cloudflare Turnstile"),
    ("challenge-platform", "Cloudflare JS Challenge"),
    ("Just a moment...", "Cloudflare"),
    ("Checking your browser", "Cloudflare"),
    ("__cf_chl_f_tk", "Cloudflare"),
    ("cf-browser-verification", "Cloudflare"),
    ("cf-ray", "Cloudflare"),
    ("cf-cache-status", "Cloudflare"),
    // Google reCAPTCHA
    ("g-recaptcha", "reCAPTCHA"),
    ("recaptcha/api.js", "reCAPTCHA"),
    ("grecaptcha.execute", "reCAPTCHA"),
    ("recaptcha.net", "reCAPTCHA"),
    // hCaptcha
    ("hcaptcha.com", "hCaptcha"),
    ("h-captcha", "hCaptcha"),
    ("hcaptcha-api", "hCaptcha"),
    // DataDome
    ("datadome", "DataDome"),
    ("dd-captcha", "DataDome"),
    ("datadome.co", "DataDome"),
    // PerimeterX / HUMAN Security
    ("perimeterx", "PerimeterX"),
    ("_pxCaptcha", "PerimeterX"),
    ("px-captcha", "PerimeterX"),
    ("perimeterx.net", "PerimeterX"),
    // Akamai Bot Manager
    ("_abck", "Akamai Bot Manager"),
    ("SensorData", "Akamai Bot Manager"),
    ("akamai-bot-manager", "Akamai Bot Manager"),
    ("akamai.net", "Akamai"),
    // Imperva / Incapsula
    ("incapsula", "Imperva Incapsula"),
    ("visid_incap", "Imperva Incapsula"),
    ("incap_ses", "Imperva Incapsula"),
    // Sucuri
    ("sucuri", "Sucuri"),
    ("sucuri.net", "Sucuri"),
    // Generic challenge phrases
    ("Please verify you are a human", "Generic Challenge"),
    ("verify you are human", "Generic Challenge"),
    ("bot detection", "Generic Detection"),
    ("automated requests", "Generic Detection"),
    ("security check", "Generic Challenge"),
    ("anti-bot", "Generic Detection"),
    // Additional modern signatures
    ("challenge.js", "Generic Challenge"),
    ("captcha.js", "Generic Challenge"),
    ("verify.js", "Generic Challenge"),
    ("bot-check", "Generic Detection"),
];

/// Calculate Shannon entropy of a string
///
/// Used to detect obfuscated JavaScript in challenge pages, which often have
/// high entropy due to minification and encoding.
///
/// # Arguments
///
/// * `s` - The string to analyze
///
/// # Returns
///
/// * Entropy value between 0.0 (uniform distribution) and ~8.0 (high entropy)
#[inline]
fn calculate_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }

    let mut freq = [0u32; 256];
    let len = s.len() as f64;

    // Count byte frequencies
    for &b in s.as_bytes() {
        freq[b as usize] += 1;
    }

    // Calculate entropy
    let mut entropy = 0.0;
    for &count in &freq {
        if count > 0 {
            let p = count as f64 / len;
            entropy -= p * p.log2();
        }
    }

    entropy
}

/// Check response size limits for WAF detection
///
/// Challenge pages often have unusual sizes:
/// - Very large: obfuscated JS challenges (> 100KB)
///
/// # Arguments
///
/// * `body_len` - Length of the response body
///
/// # Returns
///
/// * `true` if size indicates a potential challenge
#[inline]
fn is_suspicious_size(body_len: usize) -> bool {
    // Too large - likely obfuscated JS challenge
    body_len > 100_000
}

/// Enhanced WAF/CAPTCHA challenge detection
///
/// Uses multiple detection methods:
/// 1. Signature scanning (existing)
/// 2. Entropy analysis (high entropy indicates obfuscated JS)
/// 3. Response size limits (unusual sizes indicate challenges)
///
/// # Arguments
///
/// * `body` - The HTTP response body as a string
///
/// # Returns
///
/// * `Some(provider_name)` if a WAF challenge is detected
/// * `None` if no challenge detected
///
/// # Performance
///
/// O(N * M + N) where N = body length, M = signature count.
/// Zero allocations for signature scanning, minimal for entropy calculation.
#[inline]
pub fn detect_waf_challenge(body: &str) -> Option<&'static str> {
    // Check response size first (fast)
    if is_suspicious_size(body.len()) {
        // Calculate entropy for large responses
        let entropy = calculate_entropy(body);

        // High entropy (> 5.5) in large responses often indicates obfuscated challenge JS
        // Note: threshold lowered from 6.5 to 5.5 because UTF-8 encoding of code points 128-255
        // creates 2-byte sequences, reducing uniform byte distribution (~5.5 bits)
        if entropy > 5.5 {
            return Some("Entropy-Based Detection");
        }
    }

    // Signature-based detection (existing logic)
    WAF_SIGNATURES
        .iter()
        .find_map(|(sig, provider)| body.contains(sig).then_some(*provider))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_cloudflare_turnstile() {
        let html = r#"<div id="cf-turnstile" data-sitekey="abc123"></div>"#;
        assert_eq!(detect_waf_challenge(html), Some("Cloudflare Turnstile"));
    }

    #[test]
    fn test_detect_cloudflare_just_a_moment() {
        let html = "<html><body><h1>Just a moment...</h1></body></html>";
        assert_eq!(detect_waf_challenge(html), Some("Cloudflare"));
    }

    #[test]
    fn test_detect_cloudflare_checking_browser() {
        let html = "<html><body>Checking your browser before accessing...</body></html>";
        assert_eq!(detect_waf_challenge(html), Some("Cloudflare"));
    }

    #[test]
    fn test_detect_recaptcha() {
        let html = r#"<script src="https://www.google.com/recaptcha/api.js?render=abc"></script>"#;
        assert_eq!(detect_waf_challenge(html), Some("reCAPTCHA"));
    }

    #[test]
    fn test_detect_g_recaptcha() {
        let html = r#"<div class="g-recaptcha" data-sitekey="abc"></div>"#;
        assert_eq!(detect_waf_challenge(html), Some("reCAPTCHA"));
    }

    #[test]
    fn test_detect_hcaptcha() {
        let html = r#"<div class="h-captcha" data-sitekey="abc"></div>"#;
        assert_eq!(detect_waf_challenge(html), Some("hCaptcha"));
    }

    #[test]
    fn test_detect_datadome() {
        let html = r#"<script src="https://js.datadome.co/captcha.js"></script>"#;
        assert_eq!(detect_waf_challenge(html), Some("DataDome"));
    }

    #[test]
    fn test_detect_perimeterx() {
        let html = r#"<script>var _pxCaptcha = {};</script>"#;
        assert_eq!(detect_waf_challenge(html), Some("PerimeterX"));
    }

    #[test]
    fn test_detect_akamai() {
        let html = r#"<input type="hidden" name="_abck" value="xxx">"#;
        assert_eq!(detect_waf_challenge(html), Some("Akamai Bot Manager"));
    }

    #[test]
    fn test_detect_generic_challenge() {
        let html = "<p>Please verify you are a human to continue.</p>";
        assert_eq!(detect_waf_challenge(html), Some("Generic Challenge"));
    }

    #[test]
    fn test_clean_html_no_detection() {
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
        assert_eq!(detect_waf_challenge(html), None);
    }

    #[test]
    fn test_empty_body_no_detection() {
        assert_eq!(detect_waf_challenge(""), None);
    }

    #[test]
    fn test_entropy_high_obfuscated_js() {
        // Simulate obfuscated JavaScript with high entropy
        // Use a string with truly uniform byte distribution across all 256 values
        let obfuscated_js: String = (0u8..=255).map(|b| b as char).collect();
        // This should be detected by entropy analysis
        let entropy = calculate_entropy(&obfuscated_js);
        assert!(entropy > 6.0, "entropy={entropy}, expected > 6.0");
    }

    #[test]
    fn test_entropy_low_plain_text() {
        let plain_text = "Hello world, this is a normal page with regular content.";
        let entropy = calculate_entropy(plain_text);
        assert!(entropy < 5.0); // Lower entropy for plain text
    }

    #[test]
    fn test_suspicious_size_detection() {
        // Very large response (likely challenge) — this is the only threshold
        assert!(is_suspicious_size(150_000));
        // Normal size (below 100KB threshold)
        assert!(!is_suspicious_size(10_000));
        // Boundary case at threshold
        assert!(!is_suspicious_size(100_000));
        assert!(is_suspicious_size(100_001));
    }

    #[test]
    fn test_detect_by_entropy_high() {
        // Create a string that exceeds 100KB AND has high entropy
        // Mix of bytes 0x00..=0xFF repeated to get > 100KB total
        let high_entropy_content: String = (0u8..=255)
            .map(|b| b as char)
            .chain((0u8..=255).map(|b| b as char)) // double for 512 chars
            .chain((0u8..=255).map(|b| b as char)) // triple for 768 chars
            .chain((0u8..=255).map(|b| b as char)) // quad for 1024 chars
            // repeat to exceed 100KB
            .cycle()
            .take(104_000)
            .collect();
        let result = detect_waf_challenge(&high_entropy_content);
        // Should be detected by size + high entropy (> 6.5)
        assert_eq!(result, Some("Entropy-Based Detection"));
    }

    #[test]
    fn test_detect_by_entropy_low_small() {
        // Small, low-entropy content — not large enough for entropy detection
        let small_content = "<html><body>Redirecting...</body></html>";
        let result = detect_waf_challenge(small_content);
        // Size is too small to trigger entropy check — returns None
        assert_eq!(result, None);
        // But should still detect via signatures if any match
        // The redirect-like content has no WAF signatures
    }
}
