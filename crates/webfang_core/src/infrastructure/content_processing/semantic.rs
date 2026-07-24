//! Semantic content processor — pipeline scraper adapter.
//!
//! Wraps the `clean.rs` pipeline logic into a [`ContentProcessor`] impl:
//! 1. `legible::parse` — Mozilla Readability port for main content extraction
//! 2. Naive `strip_html_tags` — char-by-char `<`/`>` tag stripping
//! 3. `normalize_whitespace` — `split_whitespace().join(" ")` for Unicode-aware collapse
//!
//! Self-contained: copies the logic rather than importing private functions
//! from `clean.rs`, so each adapter is independent.

use crate::domain::content_processor::ContentProcessor;

/// Semantic content processor for the pipeline scraper.
///
/// Extracts readable content via Readability, then strips residual HTML tags
/// and normalizes whitespace. Best for RAG and readability-focused pipelines.
pub struct SemanticProcessor;

impl ContentProcessor for SemanticProcessor {
    fn process(&self, html: &str) -> String {
        let extracted = extract_readability(html);
        let no_tags = strip_html_tags(&extracted);
        normalize_whitespace(&no_tags)
    }

    fn name(&self) -> &str {
        "semantic"
    }
}

/// Extract main content using legible (Mozilla Readability).
/// Falls back to the raw HTML if extraction fails or returns empty.
fn extract_readability(html: &str) -> String {
    if html.trim().is_empty() {
        return String::new();
    }

    match legible::parse(html, None, None) {
        Ok(article) => {
            if article.content.trim().is_empty() {
                html.to_string()
            } else {
                article.content
            }
        },
        Err(_) => html.to_string(),
    }
}

/// Strip HTML tags, returning only text content.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut inside_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => inside_tag = true,
            '>' => inside_tag = false,
            c if !inside_tag => result.push(c),
            _ => {},
        }
    }
    result
}

/// Collapse runs of whitespace (spaces, tabs, newlines) into a single space
/// and trim leading/trailing whitespace.
fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<&str>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_semantic() {
        assert_eq!(SemanticProcessor.name(), "semantic");
    }

    #[test]
    fn strip_removes_tags() {
        assert_eq!(strip_html_tags("<p>hello</p>"), "hello");
        assert_eq!(strip_html_tags("<a href='x'>link</a>"), "link");
        assert_eq!(strip_html_tags("no tags"), "no tags");
        assert_eq!(strip_html_tags("<br><hr>"), "");
    }

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_whitespace("  hello  world  "), "hello world");
        assert_eq!(normalize_whitespace("a\n\nb\t\nc"), "a b c");
        assert_eq!(normalize_whitespace("single"), "single");
    }

    #[test]
    fn extract_readability_empty() {
        assert_eq!(extract_readability(""), "");
        assert_eq!(extract_readability("   \n  "), "");
    }

    #[cfg_attr(miri, ignore)] // legible/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_strips_tags_and_normalizes() {
        let p = SemanticProcessor;
        let result = p.process("<div><p>  Hello   world  </p></div>");
        assert!(!result.contains('<'), "no tags: {result}");
        assert!(result.contains("Hello"), "text preserved: {result}");
        assert!(!result.contains("  "), "no double spaces: {result}");
    }

    #[cfg_attr(miri, ignore)] // legible/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_handles_empty_input() {
        let p = SemanticProcessor;
        assert_eq!(p.process(""), "");
    }

    #[cfg_attr(miri, ignore)] // legible/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_handles_plain_text() {
        let p = SemanticProcessor;
        let result = p.process("just plain text");
        assert!(
            result.contains("just plain text"),
            "plain text preserved: {result}"
        );
    }
}
