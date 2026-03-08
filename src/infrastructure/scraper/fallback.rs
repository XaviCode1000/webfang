//! Fallback text extraction when Readability fails
//!
//! Uses htmd for HTML to Markdown conversion, with a basic
//! line-based fallback if that fails.

/// Extract text without Readability (basic HTML stripping)
///
/// This is used when Readability fails to parse the HTML.
/// It uses htmd for conversion, falling back to basic line filtering.
///
/// # Examples
///
/// ```
/// use rust_scraper::infrastructure::scraper::fallback::extract_text;
///
/// let html = "<html><body><p>Hello World</p></body></html>";
/// let text = extract_text(html);
/// assert!(text.contains("Hello World"));
/// ```
pub fn extract_text(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| {
        // If htmd fails, do a very basic strip
        html.lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_with_valid_html() {
        let html = r#"<html><body><p>Hello World</p></body></html>"#;
        let result = extract_text(html);
        assert!(result.contains("Hello World"));
        assert!(!result.contains("<html>"));
    }

    #[test]
    fn test_extract_text_with_scripts_removed() {
        let html = r#"
            <html>
                <head><script>var x = 1;</script></head>
                <body><article>Main content here</article></body>
            </html>
        "#;
        let result = extract_text(html);
        assert!(result.contains("Main content"));
        assert!(!result.contains("<script>"));
    }

    #[test]
    fn test_extract_text_empty_html() {
        let html = "";
        let result = extract_text(html);
        assert_eq!(result, "");
    }

    #[test]
    fn test_extract_text_malformed_html() {
        let html = "<div>Open div never closed<p>Paragraph";
        let result = extract_text(html);
        // Should not crash, should extract what it can
        assert!(!result.is_empty());
    }
}
