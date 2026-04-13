//! WAF/CAPTCHA challenge detection
//!
//! Detects WAF/CAPTCHA challenge pages disguised as HTTP 200 responses
//! by scanning for known signatures.

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
    // Google reCAPTCHA
    ("g-recaptcha", "reCAPTCHA"),
    ("recaptcha/api.js", "reCAPTCHA"),
    ("grecaptcha.execute", "reCAPTCHA"),
    // hCaptcha
    ("hcaptcha.com", "hCaptcha"),
    ("h-captcha", "hCaptcha"),
    // DataDome
    ("datadome", "DataDome"),
    ("dd-captcha", "DataDome"),
    // PerimeterX / HUMAN Security
    ("perimeterx", "PerimeterX"),
    ("_pxCaptcha", "PerimeterX"),
    // Akamai Bot Manager
    ("_abck", "Akamai Bot Manager"),
    ("SensorData", "Akamai Bot Manager"),
    // Generic challenge phrases
    ("Please verify you are a human", "Generic Challenge"),
    ("verify you are human", "Generic Challenge"),
    ("bot detection", "Generic Detection"),
];

/// Detect WAF/CAPTCHA challenge pages disguised as HTTP 200
///
/// Scans the response body for known WAF signatures. Returns the provider name
/// if a challenge is detected, or `None` if the content appears legitimate.
///
/// # Arguments
///
/// * `body` - The HTTP response body as a string
///
/// # Returns
///
/// * `Some(provider_name)` if a WAF challenge signature is found
/// * `None` if no challenge detected
///
/// # Performance
///
/// O(N * M) where N = body length, M = signature count (currently 19).
/// Zero allocations — uses `str::contains()` with static string slices.
/// For M > 20, consider upgrading to `aho-corasick` for O(N) DFA matching.
#[inline]
pub fn detect_waf_challenge(body: &str) -> Option<&'static str> {
    // Following **perf-iter-over-index**: iterator scan, no intermediate collections
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
}
