//! MCP content processor — MCP tools adapter.
//!
//! Wraps the `html_cleaner.rs` pipeline logic into a [`ContentProcessor`] impl.
//! Uses lol_html element handlers for boilerplate removal and attribute stripping,
//! then ASCII-state-machine whitespace normalization.
//!
//! Output retains semantic HTML tags (`<p>`, `<h1>`, etc.) — consumers that need
//! plain text should use [`super::AggressiveProcessor`] or [`super::SemanticProcessor`].

use crate::domain::content_processor::ContentProcessor;
use crate::infrastructure::converter::html_cleaner::clean_html;

/// MCP content processor for tools and API consumers.
///
/// Removes boilerplate (scripts, styles, nav, footer) via lol_html while
/// preserving semantic HTML structure. Best for MCP tools that need
/// structured HTML output with boilerplate stripped.
pub struct McpProcessor;

impl ContentProcessor for McpProcessor {
    fn process(&self, html: &str) -> String {
        clean_html(html)
    }

    fn name(&self) -> &str {
        "mcp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_mcp() {
        assert_eq!(McpProcessor.name(), "mcp");
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_removes_scripts() {
        let p = McpProcessor;
        let result = p.process(r#"<p>Hello</p><script>evil()</script><p>World</p>"#);
        assert!(!result.contains("evil"), "script removed: {result}");
        assert!(
            result.contains("Hello") && result.contains("World"),
            "text preserved: {result}"
        );
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_preserves_semantic_tags() {
        let p = McpProcessor;
        let result = p.process("<h1>Title</h1><p>Paragraph</p>");
        assert!(
            result.contains("<h1>") || result.contains("<h1 "),
            "h1 preserved: {result}"
        );
        assert!(
            result.contains("<p>") || result.contains("<p "),
            "p preserved: {result}"
        );
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_handles_empty_input() {
        let p = McpProcessor;
        assert_eq!(p.process(""), "");
    }

    #[cfg_attr(miri, ignore)] // lol_html/servo_arc aliasing incompatible with Tree Borrows
    #[test]
    fn process_removes_boilerplate() {
        let p = McpProcessor;
        let result = p.process("<nav>Menu</nav><main>Content</main><footer>Footer</footer>");
        assert!(!result.contains("Menu"), "nav removed: {result}");
        assert!(!result.contains("Footer"), "footer removed: {result}");
        assert!(result.contains("Content"), "content preserved: {result}");
    }
}
