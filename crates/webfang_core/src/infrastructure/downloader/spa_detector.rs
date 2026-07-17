//! SPA (Single-Page Application) detection.
//!
//! Analyzes HTML content to determine if a page is a static server-rendered
//! page or a JavaScript SPA that requires a headless browser for full content.
//!
//! Detection heuristics:
//! - Known SPA mount points (`#root`, `#app`, `__NEXT_DATA__`, `__NUXT__`)
//! - Insufficient static content (body too short)
//! - WAF challenge pages that impersonate SPAs

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

/// WAF challenge markers (to avoid false-positive SPA detection).
///
/// These indicate the page is a WAF challenge page, not a real SPA.
const WAF_MARKERS: &[&str] = &[
    "challenge-running",
    "Just a moment",
    "Checking your browser",
    "cf-browser-verification",
    "g-recaptcha",
    "hcaptcha",
    "data-sitekey",
];

/// Detect whether an HTML page is static content, an SPA, or a WAF challenge.
///
/// # Arguments
///
/// * `html` - Raw HTML content of the page
///
/// # Returns
///
/// A [`SpaSignal`] indicating the detection result.
///
/// # Examples
///
/// ```
/// use webfang::infrastructure::downloader::spa_detector::{detect_spa, SpaSignal};
///
/// let html = "<html><body><article><h1>Hello</h1></article></body></html>";
/// assert_eq!(detect_spa(html), SpaSignal::StaticContent);
///
/// let spa = "<html><body><div id=\"root\"></div></body></html>";
/// assert!(matches!(detect_spa(spa), SpaSignal::SpaDetected(_)));
/// ```
pub fn detect_spa(html: &str) -> SpaSignal {
    // Check for WAF markers first — these are not real SPAs
    for marker in WAF_MARKERS {
        if html.contains(marker) {
            return SpaSignal::WafBlocked;
        }
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
        assert_eq!(detect_spa(html), SpaSignal::StaticContent);
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
        let signal = detect_spa(html);
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
        let signal = detect_spa(html);
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
        let signal = detect_spa(html);
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
        let signal = detect_spa(html);
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
        let signal = detect_spa(html);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::MountPoint(ref m)) if m == "Angular app-root"
        ));
    }

    #[test]
    fn test_insufficient_content_empty() {
        let html = "";
        let signal = detect_spa(html);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientContent(0))
        ));
    }

    #[test]
    fn test_insufficient_content_short() {
        let html = "<html></html>"; // 14 bytes < 50
        let signal = detect_spa(html);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientContent(_))
        ));
    }

    #[test]
    fn test_waf_cloudflare_challenge() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Just a moment...</title></head>
<body>
  <div id="challenge-running">Checking your browser...</div>
</body>
</html>"#;
        assert_eq!(detect_spa(html), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_waf_recaptcha() {
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div class="g-recaptcha" data-sitekey="abc123"></div>
</body>
</html>"#;
        assert_eq!(detect_spa(html), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_waf_hcaptcha() {
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div class="h-captcha" data-sitekey="abc123"></div>
</body>
</html>"#;
        assert_eq!(detect_spa(html), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_waf_checked_before_spa() {
        // WAF markers should be detected even if SPA markers are present
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <div id="root"></div>
  <div id="challenge-running">Checking your browser...</div>
</body>
</html>"#;
        assert_eq!(detect_spa(html), SpaSignal::WafBlocked);
    }

    #[test]
    fn test_static_content_exact_threshold() {
        // Exactly 50 bytes should be considered static (not insufficient)
        let html = "a".repeat(50);
        assert_eq!(detect_spa(&html), SpaSignal::StaticContent);
    }

    #[test]
    fn test_static_content_below_threshold() {
        // 49 bytes should be insufficient
        let html = "a".repeat(49);
        let signal = detect_spa(&html);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientContent(49))
        ));
    }
}
