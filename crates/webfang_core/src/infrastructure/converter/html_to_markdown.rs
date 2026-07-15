//! HTML to Markdown conversion
//!
//! Uses html-to-markdown-rs crate for structure-preserving conversion.
//! HTML boilerplate (nav, sidebar, SVG, scripts) is stripped before
//! conversion for Obsidian-quality output.

use html_to_markdown_rs::{convert, CodeBlockStyle, ConversionOptions, HeadingStyle};
use tracing::warn;

/// Convert HTML to well-structured Markdown.
///
/// Pipeline:
/// 1. Remove boilerplate (scripts, nav, sidebar, SVG, page chrome)
/// 2. Convert clean HTML → Markdown with ATX headings and fenced code blocks
/// 3. Fall back to plain text if conversion fails
///
/// # Examples
///
/// ```
/// use webfang::infrastructure::converter::html_to_markdown::convert_to_markdown;
///
/// let html = "<h1>Title</h1><p>Content</p>";
/// let md = convert_to_markdown(html);
/// assert!(md.contains("# Title"));
/// ```
pub fn convert_to_markdown(html: &str) -> String {
    // Step 1: Remove boilerplate (nav, sidebar, scripts, SVG, etc.)
    let cleaned = crate::infrastructure::converter::html_cleaner::clean_html(html);

    // Step 2: Convert clean HTML → Markdown
    let options = ConversionOptions {
        heading_style: HeadingStyle::Atx,
        code_block_style: CodeBlockStyle::Backticks,
        ..Default::default()
    };

    convert(&cleaned, Some(options)).unwrap_or_else(|e| {
        warn!("HTML to Markdown conversion failed: {}, falling back", e);
        crate::infrastructure::scraper::fallback::extract_text(html)
    })
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_convert_heading() {
        let html = "<h1>Main Title</h1>";
        let md = convert_to_markdown(html);
        assert!(md.contains("# Main Title"));
    }

    #[test]
    fn test_convert_paragraph() {
        let html = "<p>This is a paragraph.</p>";
        let md = convert_to_markdown(html);
        assert!(md.contains("This is a paragraph."));
    }

    #[test]
    fn test_convert_nested_structure() {
        let html = "<article><h1>Title</h1><p>Intro</p><h2>Section</h2><p>Content</p></article>";
        let md = convert_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("## Section"));
        assert!(md.contains("Intro"));
        assert!(md.contains("Content"));
    }

    #[test]
    fn test_convert_empty_html() {
        let html = "";
        let md = convert_to_markdown(html);
        assert_eq!(md, "");
    }

    #[test]
    fn test_code_block_uses_backticks() {
        let html = "<pre><code>fn main() {}</code></pre>";
        let md = convert_to_markdown(html);
        assert!(
            md.contains("```"),
            "Expected fenced code blocks, got: {}",
            md
        );
    }

    #[test]
    fn test_boilerplate_removed() {
        let html = "<html><body><nav>Menu</nav><main><h1>Title</h1><p>Content</p></main><footer>Copyright</footer></body></html>";
        let md = convert_to_markdown(html);
        assert!(!md.contains("Menu"));
        assert!(!md.contains("Copyright"));
        assert!(md.contains("Title"));
        assert!(md.contains("Content"));
    }

    // ============================================================================
    // Error path tests
    // ============================================================================

    #[test]
    fn test_convert_links_to_markdown() {
        let html = r#"<p>Visit <a href="https://example.com">Example</a> for more info.</p>"#;
        let md = convert_to_markdown(html);
        assert!(
            md.contains("[Example](https://example.com)"),
            "link should be converted to markdown"
        );
    }

    #[test]
    fn test_convert_lists() {
        let html = r#"
            <ul>
                <li>Item 1</li>
                <li>Item 2</li>
                <li>Item 3</li>
            </ul>
            <ol>
                <li>First</li>
                <li>Second</li>
            </ol>
        "#;
        let md = convert_to_markdown(html);
        // Unordered list items should have bullet markers
        assert!(md.contains("Item 1"));
        assert!(md.contains("Item 2"));
        assert!(md.contains("Item 3"));
        // Ordered list items should be present
        assert!(md.contains("First"));
        assert!(md.contains("Second"));
    }

    #[test]
    fn test_convert_tables() {
        let html = r#"
            <table>
                <thead>
                    <tr><th>Name</th><th>Value</th></tr>
                </thead>
                <tbody>
                    <tr><td>Foo</td><td>42</td></tr>
                    <tr><td>Bar</td><td>99</td></tr>
                </tbody>
            </table>
        "#;
        let md = convert_to_markdown(html);
        // Table content should be present
        assert!(md.contains("Name"));
        assert!(md.contains("Value"));
        assert!(md.contains("Foo"));
        assert!(md.contains("Bar"));
        // Markdown tables use pipe separators
        assert!(md.contains("|"));
    }
}
