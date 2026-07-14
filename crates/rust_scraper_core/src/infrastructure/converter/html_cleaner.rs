//! HTML boilerplate removal before Markdown conversion.
//!
//! Uses Cloudflare's `lol_html` streaming rewriter for fast, CSS-selector-based
//! element removal. Zero dependency on the `selectors` crate.

use lol_html::{element, rewrite_str, RewriteStrSettings};

/// Tags to remove entirely (element + all content).
const TAGS_TO_REMOVE: &[&str] = &[
    "script", "style", "noscript", "form", "iframe", "object", "embed", "svg", "canvas", "video",
    "audio", "nav", "header", "footer", "aside",
];

/// CSS selectors for elements to remove (class-based, attribute-based).
const SELECTORS_TO_REMOVE: &[&str] = &[
    // Starlight/Astro navigation and sidebar
    ".site-title",
    ".global-nav",
    ".global-nav-list",
    ".mobile-menu-wrapper",
    ".right-sidebar",
    ".right-sidebar-container",
    ".mobile-toc",
    ".sl-sidebar",
    ".sl-mobile-toc",
    // Search and feedback
    ".search",
    ".site-search",
    ".social-icons",
    ".page-feedback",
    ".feedback",
    // Breadcrumb and pagination
    ".sl-breadcrumbs",
    ".pagination",
    // Accessibility-hidden elements
    "[class*='sr-only']",
    "[aria-hidden='true']",
    "[hidden]",
    // Copy-to-clipboard and utility buttons
    ".copy-markdown-btn",
    ".copy-code-button",
    // Skip links
    ".skip-link",
];

/// Attributes to preserve — all others are stripped from elements.
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
/// Returns the cleaned HTML as a string.
pub fn clean_html(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }

    // Build element handlers: one per selector (tag or CSS)
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

    // Attribute stripping handler — runs on ALL elements (*)
    handlers.push(element!("*", |el| {
        let attr_names: Vec<String> = el
            .attributes()
            .iter()
            .map(|attr| attr.name().to_string())
            .collect();

        for name in attr_names {
            if !PRESERVED_ATTRS.contains(&name.as_str()) {
                el.remove_attribute(&name);
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
            tracing::warn!("error reescribiendo HTML con lol_html: {e}");
            html.to_string()
        },
    }
}

/// Collapse consecutive whitespace into single spaces.
fn normalize_whitespace(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_whitespace = false;

    for ch in html.chars() {
        if ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r' {
            if !in_whitespace {
                result.push(' ');
                in_whitespace = true;
            }
        } else {
            result.push(ch);
            in_whitespace = false;
        }
    }

    result
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_clean_removes_scripts() {
        let html = "<html><body><script>alert(1)</script><p>Hello</p></body></html>";
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("<script>"));
        assert!(cleaned.contains("Hello"));
    }

    #[test]
    fn test_clean_removes_svg() {
        let html =
            "<html><body><nav><svg>icon</svg></nav><article><h1>Title</h1></article></body></html>";
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("<svg>"));
        assert!(!cleaned.contains("<nav>"));
    }

    #[test]
    fn test_clean_preserves_content() {
        let html = "<html><body><nav>Menu</nav><main><h1>Article</h1><p>Content here</p></main></body></html>";
        let cleaned = clean_html(html);
        assert!(cleaned.contains("Article"));
        assert!(cleaned.contains("Content here"));
        assert!(!cleaned.contains("Menu"));
    }

    #[test]
    fn test_clean_empty_html() {
        let html = "";
        let cleaned = clean_html(html);
        assert!(cleaned.is_empty());
    }

    #[test]
    fn test_clean_removes_css_selectors() {
        let html = r#"
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
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("global-nav"));
        assert!(!cleaned.contains("site-title"));
        assert!(cleaned.contains("Main Content"));
        assert!(cleaned.contains("This should remain"));
    }

    #[test]
    fn test_clean_preserves_href_attribute() {
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
    fn test_clean_whitespace_normalization() {
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
