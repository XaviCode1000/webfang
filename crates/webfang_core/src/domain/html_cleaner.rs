//! Pure HTML cleaner — domain-owned, no infrastructure import.
//!
//! Uses `lol_html` streaming rewriter. Mirrors
//! `infrastructure::converter::html_cleaner` but lives in domain so
//! `application::spa_detection` can import without violating
//! `application → infrastructure`.

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

/// Clean HTML by removing boilerplate.
///
/// Pure function — no trait, no I/O. Returns cleaned HTML.
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
        Err(_) => html.to_string(),
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
    fn clean_empty_returns_empty() {
        assert_eq!(clean_html(""), "");
        let html2 = "<html><body><nav>Menu</nav><main><h1>Article</h1></main></body></html>";
        let cleaned2 = clean_html(html2);
        assert!(cleaned2.contains("Article"));
        assert!(!cleaned2.contains("Menu"));
    }

    #[test]
    fn clean_strips_javascript_scheme() {
        let html = r#"<html><body><a href="javascript:alert(1)">click</a></body></html>"#;
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("javascript:"));
        let safe = r#"<a href="https://example.com">ok</a>"#;
        assert!(clean_html(safe).contains("https://example.com"));
    }

    #[test]
    fn clean_removes_css_selectors() {
        let html = r#"<html><body><div class="global-nav">nav</div><main><p>keep</p></main></body></html>"#;
        let cleaned = clean_html(html);
        assert!(!cleaned.contains("global-nav"));
        assert!(cleaned.contains("keep"));
    }
}
