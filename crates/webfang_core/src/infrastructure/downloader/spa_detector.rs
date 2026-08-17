//! SPA (Single-Page Application) detection.
//!
//! Analyzes HTML content to determine if a page is a static server-rendered
//! page or a JavaScript SPA that requires a headless browser for full content.
//!
//! Detection heuristics (in order):
//! - WAF challenge pages that impersonate SPAs (checked first, REQ-WAF-10)
//! - Insufficient VISIBLE TEXT: the extracted text (scripts/styles stripped)
//!   is counted in characters, aligned with
//!   [`crate::application::spa_detection::MIN_CONTENT_CHARS`]. A fat JS
//!   shell (thousands of raw HTML bytes, near-zero readable text, e.g.
//!   `quotes.toscrape.com/js/`) must escalate even though its raw byte count
//!   is large (#758).
//! - Known SPA mount points (`#root`, `#app`, `__NEXT_DATA__`, `__NUXT__`) —
//!   only consulted when text is insufficient, as an enriched reason. Pages
//!   with substantial text are static even if they embed hydration markers
//!   (SSR), mirroring the char-gate-first semantics of the application layer.

use crate::infrastructure::http::waf_engine::{InspectionContext, WafInspector};

/// Signal indicating whether a page is static, an SPA, or WAF-blocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaSignal {
    /// Page has sufficient visible text content — no JS rendering needed.
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
    /// Page visible text is too short to contain meaningful static content.
    /// The `usize` is the non-whitespace character count of the extracted
    /// visible text (NOT raw HTML bytes — #758).
    InsufficientText(usize),
}

/// Minimum visible-text length (in non-whitespace characters) to consider a
/// page as having meaningful static content. Aligned with
/// [`crate::application::spa_detection::MIN_CONTENT_CHARS`] so both layers
/// share one semantic for "enough content" (#758).
const MIN_VISIBLE_CHARS: usize = 50;

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

/// Tags whose text descendants are NOT visible content. Scripts and styles
/// carry the bulk of JS shells; `noscript`/`template` content is inert markup
/// a browser never renders as page text.
const INVISIBLE_TEXT_TAGS: [&str; 4] = ["script", "style", "noscript", "template"];

// WAF challenge classification is delegated to the shared inspection verdict
// (REQ-WAF-10) — the former local `WAF_MARKERS` list was folded into the
// unified signature registry in `waf_engine` and removed here.

/// Count the non-whitespace characters of visible text in an HTML document.
///
/// Parses the document and walks text nodes, excluding descendants of
/// [`INVISIBLE_TEXT_TAGS`]. This is the infra-tier semantic aligned with the
/// application layer's extracted-text char count (#758): raw HTML byte length
/// is a broken proxy because JS shells serve kilobytes of markup with
/// near-zero readable text.
fn visible_text_chars(html: &str) -> usize {
    let document = scraper::Html::parse_document(html);
    document
        .root_element()
        .descendants()
        .filter(|node| {
            node.value().is_text()
                && !node.ancestors().any(|ancestor| {
                    ancestor
                        .value()
                        .as_element()
                        .is_some_and(|element| INVISIBLE_TEXT_TAGS.contains(&element.name()))
                })
        })
        .filter_map(|node| node.value().as_text())
        .flat_map(|text| text.chars())
        .filter(|c| !c.is_whitespace())
        .count()
}

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
/// // Substantial visible text → static, even with hydration markers (SSR).
/// let html = "<html><body><article><h1>Hello</h1><p>This page carries enough visible text to be considered static content by the detector.</p></article></body></html>";
/// assert_eq!(detect_spa(html, false), SpaSignal::StaticContent);
///
/// // A JS shell: fat markup, near-zero visible text → SPA.
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

    // Visible-text gate (#758): count extracted text characters, not raw HTML
    // bytes. A fat JS shell (quotes.toscrape.com/js/: ~5.8 KB raw, ~0 text)
    // must escalate; an SSR page with hydration markers but substantial text
    // must NOT.
    let text_chars = visible_text_chars(html);
    if text_chars >= MIN_VISIBLE_CHARS {
        return SpaSignal::StaticContent;
    }

    // Insufficient text — enrich the reason with a mount-point marker when
    // one is present (preserves MountPoint diagnostics for known shells).
    for (marker, description) in SPA_MARKERS {
        if html.contains(marker) {
            return SpaSignal::SpaDetected(SpaReason::MountPoint(description.to_string()));
        }
    }

    SpaSignal::SpaDetected(SpaReason::InsufficientText(text_chars))
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
    fn test_insufficient_text_empty() {
        let html = "";
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientText(0))
        ));
    }

    #[test]
    fn test_insufficient_text_short() {
        let html = "<html></html>"; // 0 visible chars < 50
        let signal = detect_spa(html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientText(_))
        ));
    }

    /// #758 regression: a FAT JS shell (thousands of raw bytes, near-zero
    /// visible text, no known mount-point marker) must escalate. The old
    /// raw-byte heuristic classified this as `StaticContent` and the hybrid
    /// router never reached Obscura (quotes.toscrape.com/js/ case).
    #[test]
    fn test_fat_shell_without_markers_escalates() {
        let fat_script = "var x = 1;".repeat(600); // ~6 KB of raw markup
        let html = format!(
            "<!DOCTYPE html><html><head><title>JS App</title>\
             <script>{fat_script}</script></head><body></body></html>"
        );
        assert!(html.len() > 5_000, "fixture must be a fat raw-HTML shell");
        let signal = detect_spa(&html, false);
        assert!(
            matches!(
                signal,
                SpaSignal::SpaDetected(SpaReason::InsufficientText(n)) if n < 50
            ),
            "fat shell with near-zero visible text must escalate, got: {signal:?}"
        );
    }

    /// #758: script/style/noscript/template text must NOT count as visible
    /// content — only rendered page text does.
    #[test]
    fn test_invisible_tags_do_not_count_as_text() {
        let html = format!(
            "<html><body><style>{}</style><noscript>{}</noscript>\
             <template>{}</template><p>short</p></body></html>",
            "a".repeat(100),
            "b".repeat(100),
            "c".repeat(100),
        );
        let signal = detect_spa(&html, false);
        assert!(
            matches!(
                signal,
                SpaSignal::SpaDetected(SpaReason::InsufficientText(n)) if n < 50
            ),
            "text inside invisible tags must not satisfy the threshold, got: {signal:?}"
        );
    }

    /// #758: an SSR page that embeds hydration markers (`__NEXT_DATA__`) but
    /// ships substantial visible text must NOT escalate — the char gate runs
    /// before marker inspection, aligned with `application::spa_detection`.
    #[test]
    fn test_ssr_with_content_is_static() {
        let html = r#"<!DOCTYPE html>
<html>
<body>
  <script id="__NEXT_DATA__" type="application/json">{"props":{"page":"home"}}</script>
  <div id="root"><article><h1>Server rendered</h1><p>This content was hydrated server-side and is fully readable without executing any JavaScript at all.</p></article></div>
</body>
</html>"#;
        assert_eq!(detect_spa(html, false), SpaSignal::StaticContent);
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
        // static classification instead of a false-positive WafBlocked. The
        // prose carries enough visible text to stay static under the #758
        // text gate.
        let html = r#"<html><body><article><p>This site is protected by cloudflare and this sentence is long enough to pass the visible text threshold.</p></article></body></html>"#;
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
        // Exactly 50 visible chars should be considered static (not insufficient)
        let html = format!("<html><body>{}</body></html>", "a".repeat(50));
        assert_eq!(detect_spa(&html, false), SpaSignal::StaticContent);
    }

    #[test]
    fn test_static_content_below_threshold() {
        // 49 visible chars should be insufficient
        let html = format!("<html><body>{}</body></html>", "a".repeat(49));
        let signal = detect_spa(&html, false);
        assert!(matches!(
            signal,
            SpaSignal::SpaDetected(SpaReason::InsufficientText(49))
        ));
    }
}
