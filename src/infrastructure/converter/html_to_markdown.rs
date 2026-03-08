//! HTML to Markdown conversion
//!
//! Uses html-to-markdown-rs crate for structure-preserving conversion.

use html_to_markdown_rs::{convert, ConversionOptions, HeadingStyle};
use tracing::warn;

/// Convert HTML to well-structured Markdown
///
/// Uses ATX-style headings (# ## ###) for better readability.
/// Falls back to plain text if conversion fails.
///
/// # Examples
///
/// ```
/// use rust_scraper::infrastructure::converter::html_to_markdown::convert_to_markdown;
///
/// let html = "<h1>Title</h1><p>Content</p>";
/// let md = convert_to_markdown(html);
/// assert!(md.contains("# Title"));
/// ```
pub fn convert_to_markdown(html: &str) -> String {
    let options = ConversionOptions {
        heading_style: HeadingStyle::Atx,
        ..Default::default()
    };

    convert(html, Some(options)).unwrap_or_else(|e| {
        warn!("HTML to Markdown conversion failed: {}, falling back", e);
        crate::infrastructure::scraper::fallback::extract_text(html)
    })
}

#[cfg(test)]
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
}
