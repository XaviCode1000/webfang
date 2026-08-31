//! Pure HTML cleaner — domain-owned, no infrastructure import.
//!
//! Uses `lol_html` streaming rewriter. This module is the **single owner** of
//! the cleaner. It lives in domain so `application::spa_detection` can import
//! it without violating `application → infrastructure`.
//!
//! `infrastructure::converter::html_cleaner` is a re-export of this module,
//! kept only so the historical infrastructure path keeps resolving for its
//! consumers. New code should call [`clean_html`] here directly.

use lol_html::{element, rewrite_str, RewriteStrSettings};

/// Tags to remove entirely.
const TAGS_TO_REMOVE: &[&str] = &[
    "script", "style", "noscript", "form", "iframe", "object", "embed", "svg", "canvas", "video",
    "audio", "nav", "header", "footer", "aside",
];

/// CSS selectors for elements to remove.
const SELECTORS_TO_REMOVE: &[&str] = &[
    ".site-title",
    ".global-nav",
    ".global-nav-list",
    ".mobile-menu-wrapper",
    ".right-sidebar",
    ".right-sidebar-container",
    ".mobile-toc",
    ".sl-sidebar",
    ".sl-mobile-toc",
    ".search",
    ".site-search",
    ".social-icons",
    ".page-feedback",
    ".feedback",
    ".sl-breadcrumbs",
    ".pagination",
    "[class*='sr-only']",
    "[aria-hidden='true']",
    "[hidden]",
    ".copy-markdown-btn",
    ".copy-code-button",
    ".skip-link",
];

/// Attributes to preserve.
const PRESERVED_ATTRS: &[&str] = &["href", "src", "alt", "id", "class", "dir", "code"];

/// Clean HTML by removing boilerplate (nav, sidebar, scripts, SVGs).
///
/// Removes:
/// - `script`, `style`, `noscript` (code and styles)
/// - `form`, `iframe`, `object`, `embed` (interactive)
/// - `svg`, `canvas`, `video`, `audio` (media)
/// - `nav`, `header`, `footer`, `aside` (page chrome)
/// - Elements matching CSS selectors (sidebars, search, breadcrumbs)
/// - Strips non-preserved attributes (keeps href, src, alt, id, class, dir, code)
///
/// Pure function — no trait, no I/O. Returns the cleaned HTML as a string.
#[must_use]
pub fn clean_html(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }

    let mut handlers: Vec<_> = TAGS_TO_REMOVE
        .iter()
        .chain(SELECTORS_TO_REMOVE.iter())
        .map(|selector| {
            let sel = *selector;
            element!(sel, |el| {
                el.remove();
                Ok(())
            })
        })
        .collect();

    handlers.push(element!("*", |el| {
        let attr_names: Vec<String> = el
            .attributes()
            .iter()
            .map(|attr| attr.name().to_string())
            .collect();
        for name in attr_names {
            if !PRESERVED_ATTRS.contains(&name.as_str()) {
                el.remove_attribute(&name);
                continue;
            }
            if name == "href" || name == "src" {
                if let Some(value) = el.get_attribute(&name) {
                    let trimmed = value.trim_start();
                    let scheme = trimmed
                        .split_once(':')
                        .map(|(s, _)| s.to_ascii_lowercase())
                        .unwrap_or_default();
                    if scheme == "javascript" {
                        el.remove_attribute(&name);
                        tracing::debug!("Stripped javascript: {} attribute", name);
                    }
                }
            }
        }
        Ok(())
    }));

    match rewrite_str(
        html,
        RewriteStrSettings {
            element_content_handlers: handlers,
            ..RewriteStrSettings::new()
        },
    ) {
        Ok(result) => normalize_whitespace(&result),
        Err(e) => {
            tracing::warn!("error rewriting HTML with lol_html: {e}");
            html.to_string()
        },
    }
}

fn normalize_whitespace(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_ws = false;
    for ch in html.chars() {
        if ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r' {
            if !in_ws {
                result.push(' ');
                in_ws = true;
            }
        } else {
            result.push(ch);
            in_ws = false;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_scripts_and_preserves_content() {
        let html = "<html><body><script>alert(1)</script><p>Hello</p></body></html>";
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("<script>"));
        assert!(cleaned.contains("Hello"));
    }
    #[test]
    fn clean_removes_svg() {
        let html =
            "<html><body><nav><svg>icon</svg></nav><article><h1>Title</h1></article></body></html>";
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("<svg>"));
        assert!(!cleaned.contains("<nav>"));
    }

    #[test]
    fn clean_empty_returns_empty() {
        assert_eq!(clean_html(""), "");
        let html2 =
"<html><body><nav>Menu</nav><main><h1>Article</h1><p>Content here</p></main></body></html>";
        let cleaned2 = clean_html(html2);
        assert!(cleaned2.contains("Article"));
        assert!(cleaned2.contains("Content here"));
        assert!(!cleaned2.contains("Menu"));
    }

    /// Bug #10 regression: `javascript:` URLs in href/src MUST be stripped
    /// to prevent XSS when cleaned HTML is re-rendered (issue #590).
    #[test]
    fn clean_strips_javascript_scheme() {
        let html = r#"<html><body><a href="javascript:alert(1)">click</a><img src="javascript:alert(2)"></body></html>"#;
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("javascript:"),
            "javascript: scheme must be stripped: {cleaned}"
        );
        let safe = r#"<html><body><a href="https://example.com/page">safe</a><img src="https://example.com/img.png"></body></html>"#;
        let cleaned = clean_html(safe);
        assert!(
            cleaned.contains("https://example.com/page"),
            "https href must be preserved: {cleaned}"
        );
        assert!(
            cleaned.contains("https://example.com/img.png"),
            "https src must be preserved: {cleaned}"
        );
    }
    #[test]
    fn clean_strips_javascript_scheme_case_insensitive() {
        let html = r#"<a href="  JAVASCRIPT:alert(1)">click</a>"#;
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("alert"),
            "case-insensitive javascript: must be stripped: {cleaned}"
        );
    }
    #[test]
    fn clean_preserves_href_attribute() {
        let html = r#"<html><body><a href="https://example.com" onclick="alert(1)" class="link">Click</a></body></html>"#;
        let cleaned = clean_html(html);
        assert!(cleaned.contains("href="), "href should be preserved");
        assert!(
            cleaned.contains("https://example.com"),
            "href URL should be preserved"
        );
        assert!(!cleaned.contains("onclick"), "onclick should be stripped");
    }

    #[test]
    fn clean_removes_css_selectors() {
        // Bare `.global-nav` on a plain div.
        let html = r#"<html><body><div class="global-nav">nav</div><main><p>keep</p></main></body></html>"#;
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("global-nav"));
        assert!(cleaned.contains("keep"));

        // Nested Starlight/Astro chrome: the selectors must go with their
        // containers while the article survives.
        let nested = r#"
<html>
<body>
<nav class="global-nav">
<span class="site-title">My Site</span>
<ul class="global-nav-list">
<li><a href="/">Home</a></li>
</ul>
</nav>
<main>
<h1>Main Content</h1>
<p>This should remain</p>
</main>
</body>
</html>
        "#;
        let cleaned = clean_html(nested);
        assert!(!cleaned.contains("global-nav"));
        assert!(!cleaned.contains("site-title"));
        assert!(cleaned.contains("Main Content"));
        assert!(cleaned.contains("This should remain"));
    }
    #[test]
    fn clean_whitespace_normalization() {
        let html =
            "<html><body><p>  Too   many    spaces  </p><p>\n\n\tNewlines\t\t</p></body></html>";
        let cleaned = clean_html(html);
        assert!(
            !cleaned.contains("   "),
            "multiple spaces should be collapsed"
        );
        assert!(
            !cleaned.contains("\n\n"),
            "multiple newlines should be collapsed"
        );
    }
}
