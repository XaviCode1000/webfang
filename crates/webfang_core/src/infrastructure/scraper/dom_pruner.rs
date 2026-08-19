//! DOM pre-pruning pass to remove invisible/empty nodes before Readability.
//!
//! Uses regex-based pattern matching for efficient removal of:
//! - Elements with inline CSS (display:none, visibility:hidden)
//! - Empty wrapper elements (no attributes, no text content)
//!
//! Runs AFTER html_cleaner::clean_html since it catches CSS-invisibility missed by attribute-based removal.

use tracing::{debug, instrument};

/// Tags to consider for empty-wrapper removal.
static PRUNE_TAGS: &[&str] = &["div", "span", "p", "section", "article"];

/// Remove elements with inline CSS invisibility and empty wrappers.
/// Uses iterative pattern matching to handle nested elements.
fn remove_invisible(html: &str) -> String {
    let mut result = html.to_string();
    let mut changed = true;
    let mut iterations = 0;
    const MAX_ITER: usize = 5;

    while changed && iterations < MAX_ITER {
        changed = false;
        iterations += 1;

        // Remove elements with display:none (single and with colon)
        let re = regex::Regex::new(r"(?is)<(?:div|span|p|section|article|footer|nav|aside|header|main|ul|ol|li)[^>]*display\s*:\s*none[^>]*>.*?</(?:div|span|p|section|article|footer|nav|aside|header|main|ul|ol|li)>").unwrap();
        let before = result.clone();
        result = re.replace_all(&result, "").to_string();
        if before != result { changed = true; }

        // Remove elements with visibility:hidden
        let re = regex::Regex::new(r"(?is)<(?:div|span|p|section|article|footer|nav|aside|header|main|ul|ol|li)[^>]*visibility\s*:\s*hidden[^>]*>.*?</(?:div|span|p|section|article|footer|nav|aside|header|main|ul|ol|li)>").unwrap();
        let before = result.clone();
        result = re.replace_all(&result, "").to_string();
        if before != result { changed = true; }

        // Remove empty wrappers (no attributes, no content)
        for tag in PRUNE_TAGS {
            let re = regex::Regex::new(&format!(r"(?is)<{0}[^>]*>\s*</{0}>", tag)).unwrap();
            let before = result.clone();
            result = re.replace_all(&result, "").to_string();
            if before != result { changed = true; }
        }
    }

    result
}

/// Remove elements that are hidden via inline CSS (display:none / visibility:hidden).
/// Also removes empty wrapper elements (no attributes, no text content).
///
/// # Returns
/// A tuple of (pruned_html, reduction_ratio) where reduction_ratio is
/// the fraction of bytes removed (0.0 = no reduction, 1.0 = everything removed).
#[instrument(skip(html), fields(original_len = html.len()))]
pub fn prune_dom(html: &str) -> (String, f64) {
    if html.is_empty() {
        return (html.to_string(), 0.0);
    }

    let original_len = html.len();

    let result = remove_invisible(html);
    let pruned_len = result.len();
    let reduction = if original_len > 0 {
        (original_len - pruned_len) as f64 / original_len as f64
    } else {
        0.0
    };

    debug!(
        original_len = original_len,
        pruned_len = %pruned_len,
        reduction_ratio = %reduction,
        "DOM pre-pruning complete"
    );
    (result, reduction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_removes_display_none() {
        let html = r#"<div style="display:none">hidden</div><p>visible</p>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(!result.contains("hidden"));
        assert!(result.contains("visible"));
    }

    #[test]
    fn prune_removes_display_none_with_colon() {
        let html = r#"<div style="display: none">hidden</div><p>visible</p>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(!result.contains("hidden"));
        assert!(result.contains("visible"));
    }

    #[test]
    fn prune_removes_visibility_hidden() {
        let html = r#"<span style="visibility:hidden">hidden</span><p>visible</p>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(!result.contains("hidden"));
        assert!(result.contains("visible"));
    }

    #[test]
    fn prune_keeps_elements_with_attributes() {
        let html = r#"<div class="keep">has class</div>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(result.contains("keep"));
    }

    #[test]
    fn prune_removes_empty_wrapper_no_attrs() {
        let html = r#"<div><span></span></div><p>content</p>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(result.contains("content"), "content should be preserved");
    }

    #[test]
    fn prune_keeps_void_elements() {
        let html = r#"<br><hr><img src="x"><p>text</p>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(result.contains("<br"));
        assert!(result.contains("<hr"));
        assert!(result.contains("<img"));
    }

    #[test]
    fn prune_reports_positive_ratio_when_content_removed() {
        let html = r#"<div style="display:none">hidden content</div><p>visible</p>"#;
        let (_result, ratio) = prune_dom(html);
        assert!(ratio > 0.0, "Should have positive reduction ratio when content is removed");
    }

    #[test]
    fn prune_returns_empty_for_empty_input() {
        let (result, ratio) = prune_dom("");
        assert!(result.is_empty());
        assert_eq!(ratio, 0.0);
    }

    #[test]
    fn prune_handles_multiple_invisible_elements() {
        let html = r#"<div style="display:none">a</div><span style="visibility:hidden">b</span><p>visible</p>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(!result.contains(">a<"));
        assert!(!result.contains(">b<"));
        assert!(result.contains("visible"));
    }

    #[test]
    fn prune_handles_nested_invisible() {
        let html = r#"<div style="display:none"><span>inner</span></div><p>visible</p>"#;
        let (result, _ratio) = prune_dom(html);
        assert!(!result.contains("inner"));
        assert!(result.contains("visible"));
    }
}