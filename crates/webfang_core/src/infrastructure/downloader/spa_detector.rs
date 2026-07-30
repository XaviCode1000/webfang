//! SPA (Single-Page Application) detection.
//!
//! Analyzes HTML content to determine if a page is a static server-rendered
//! page or a JavaScript SPA that requires a headless browser for full content.
//!
//! Detection heuristics:
//! - Known SPA mount points (`#root`, `#app`, `__NEXT_DATA__`, `__NUXT__`)
//! - Insufficient static content (body too short)
//! - WAF challenge pages that impersonate SPAs

use crate::infrastructure::http::waf_engine::{InspectionContext, WafInspector};

/// Signal indicating whether a page is static, an SPA, or WAF-blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaSignal {
    /// Page has sufficient static HTML content — no JS rendering needed.
    StaticContent,
    /// Page is detected as an SPA with the given reason.
    SpaDetected(SpaReason),
    /// Page is a WAF challenge (Cloudflare, reCAPTCHA, etc.) — not a real SPA.
    WafBlocked,
}

/// Why a page was detected as an SPA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaReason {
    /// Known SPA mount point found (e.g., `#root`, `#app`, `__NEXT_DATA__`, `__NUXT__`).
    MountPoint(String),
    /// Page body is too short to contain meaningful static content.
    /// The `usize` is the content length in bytes.
    InsufficientContent(usize),
}

/// Minimum HTML content length (in bytes) to consider a page as having
/// meaningful static content. Below this threshold, the page is likely
/// an SPA shell with minimal markup.
const MIN_CONTENT_LENGTH: usize = 50;

/// Known SPA mount point markers.
///
/// Each entry is a (marker, description) pair. Markers are checked as
/// substrings in the HTML body.
const SPA_MARKERS: &[(&str, &str)] = &[
    // React / Next.js
    ("id=\"root\"", "React #root"),
    ("id=\"app\"", "Vue/React #app"),
    ("__NEXT_DATA__", "Next.js"),
    // Nuxt.js
    ("__NUXT__", "Nuxt.js"),
    // Vue
    ("id=\"app\"", "Vue #app"),
    // Angular
    ("<app-root>", "Angular app-root"),
    // Remix
    ("__REMIX_DATA__", "Remix"),
];

// WAF challenge classification is delegated to the shared inspection verdict
// (REQ-WAF-10) — the former local `WAF_MARKERS` list was folded into the
// unified signature registry in `waf_engine` and removed here.

/// Detect whether an HTML page is static content, an SPA, or a WAF challenge.
///
/// # Arguments
///
/// * `html` - Raw HTML content of the page
/// * `ignore_waf` - Bypass WAF classification (REQ-WAF-07). When `true`, the
///   inspection yields a clean verdict and the page is classified by normal
///   spa/static logic instead of [`SpaSignal::WafBlocked`].
///
/// # Returns
///
/// A [`SpaSignal`] indicating the detection result.
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::downloader::spa_detector::{detect_spa, SpaSignal};
///
/// let html = "<html><body><article><h1>Hello</h1></article></body></html>";
/// assert_eq!(detect_spa(html, false), SpaSignal::StaticContent);
///
/// let spa = "<html><body><div id=\"root\"></div></body></html>";
/// assert!(matches!(detect_spa(spa, false), SpaSignal::SpaDetected(_)));
/// ```
pub fn detect_spa(html: &str, ignore_waf: bool) -> SpaSignal {
    // Check for WAF challenges first via the shared inspection verdict
    // (REQ-WAF-10) — challenge pages are not real SPAs. Degraded context (no
    // HTTP status/content-type): only Challenge-tier (T1) markers classify a
    // challenge, so bare Fingerprint vendor names no longer cause false
    // positives (issue #346). `ignore_waf` short-circuits to a clean verdict
    // (REQ-WAF-07) so an opted-out caller never aborts on the spa path (W1).
    let ctx = InspectionContext {
        ignore_waf,
        ..InspectionContext::default()
    };
    let verdict = WafInspector::inspect(html, &ctx);
    if verdict.is_blocked {
        return SpaSignal::WafBlocked;
    }

    // Check content length
    if html.len() < MIN_CONTENT_LENGTH {
        return SpaSignal::SpaDetected(SpaReason::InsufficientContent(html.len()));
    }

    // Check for SPA mount points
    for (marker, description) in SPA_MARKERS {
        if html.contains(marker) {
            return SpaSignal::SpaDetected(SpaReason::MountPoint(description.to_string()));
        }
    }

    SpaSignal::StaticContent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_static_content_normal_page() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body>
  <article>
    <h1>Hello World</h1>
    <p>This is a normal static page with enough content.</p>
  </article>
</body>
</html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::StaticContent);
    }

    #[test]
    fn test_spa_react_root() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>React App</title></head>
<body>
  <div id="root"></div>
  <script src="/static/js/bundle.js"></script>
</body>
</html>"#;
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::MountPoint(ref m)) if m == "React #root"
        ));
    }

    #[test]
    fn test_spa_vue_app() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Vue App</title></head>
<body>
  <div id="app"></div>
  <script src="/js/app.js"></script>
</body>
</html>"#;
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::MountPoint(_))
        ));
    }

    #[test]
    fn test_spa_next_js() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Next App</title></head>
<body>
  <div id="__next"></div>
  <script id="__NEXT_DATA__" type="application/json">{"props":{}}</script>
</body>
</html>"#;
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::MountPoint(ref m)) if m == "Next.js"
        ));
    }

    #[test]
    fn test_spa_nuxt() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Nuxt App</title></head>
<body>
  <div id="__nuxt"></div>
  <script>window.__NUXT__={}</script>
</body>
</html>"#;
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::MountPoint(ref m)) if m == "Nuxt.js"
        ));
    }

    #[test]
    fn test_spa_angular() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Angular App</title></head>
<body>
  <app-root></app-root>
</body>
</html>"#;
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::MountPoint(ref m)) if m == "Angular app-root"
        ));
    }

    #[test]
    fn test_insufficient_content_empty() {
        let html = "";
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientContent(0))
        ));
    }

    #[test]
    fn test_insufficient_content_short() {
        let html = "<html></html>"; // 14 bytes < 50
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientContent(_))
        ));
    }

    #[test]
    fn test_waf_cloudflare_challenge() {
        // Challenge-tier (T1) markers classify a challenge via the shared
        // inspection verdict (REQ-WAF-10) — not a real SPA.
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Just a moment...</title></head>
<body>
  <div id="challenge-running">Checking your browser...</div>
</body>
</html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_waf_recaptcha() {
        // g-recaptcha is a Challenge-tier (T1) widget marker → challenge verdict.
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div class="g-recaptcha" data-sitekey="abc123"></div>
</body>
</html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_waf_hcaptcha() {
        // h-captcha is a Challenge-tier (T1) widget marker → challenge verdict
        // (data-sitekey alone is only Fingerprint-tier and would not block).
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div class="h-captcha" data-sitekey="abc123"></div>
</body>
</html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_waf_checked_before_spa() {
        // WAF challenges are detected even if SPA markers are present
        // (now via the shared inspection verdict — REQ-WAF-10).
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div id="root"></div>
  <div id="challenge-running">Checking your browser...</div>
</body>
</html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_waf_t2_fingerprint_alone_not_challenge() {
        // REQ-WAF-10 / #346: a bare Fingerprint-tier marker (a vendor name in
        // prose) is NOT a challenge in degraded mode — it falls through to normal
        // static classification instead of a false-positive WafBlocked.
        let html = r#"<html><body><article><p>This site is protected by cloudflare.</p></article></body></html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::StaticContent);
    }

    #[test]
    fn test_detect_spa_ignore_waf_false_t1_blocked() {
        // Mirror pinning current behavior: ignore_waf=false classifies a T1
        // challenge as WafBlocked (REQ-WAF-07).
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div id="challenge-running">Checking your browser...</div>
</body>
</html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_detect_spa_ignore_waf_true_t1_not_blocked() {
        // REQ-WAF-07 (W1): ignore_waf=true short-circuits the inspection to a
        // clean verdict, so a genuine T1 challenge is NOT WafBlocked — it falls
        // through to normal spa/static classification.
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div id="challenge-running">Checking your browser...</div>
</body>
</html>"#;
        let signal = detect_spa(html, true);
        assert_ne!(signal, SpaSignal::WafBlocked);
    }

    #[test]
    fn test_detect_spa_ignore_waf_true_t1_with_spa_marker_escalates() {
        // Triangulation: with ignore_waf=true a T1 challenge that ALSO carries
        // an SPA mount point is classified by normal spa logic (SpaDetected),
        // not WafBlocked — the page is treated per the regular spa path. The
        // ignore_waf=false mirror is test_waf_checked_before_spa (WafBlocked).
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div id="root"></div>
  <div id="challenge-running">Checking your browser...</div>
</body>
</html>"#;
        assert!(matches!(
            detect_spa(html, true),
            SpaSignal::SpaDetected(SpaReason::MountPoint(_))
        ));
    }

    #[test]
    fn test_static_content_exact_threshold() {
        // Exactly 50 bytes should be considered static (not insufficient)
        let html = "a".repeat(50);
        assert_eq!(detect_spa(&html, false), SpaSignal::StaticContent);
    }

    #[test]
    fn test_static_content_below_threshold() {
        // 49 bytes should be insufficient
        let html = "a".repeat(49);
        let signal = detect_spa(&html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientContent(49))
        ));
    }
}
