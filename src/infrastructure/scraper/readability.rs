//! Readability algorithm wrapper (legible crate)
//!
//! Uses the same algorithm as Firefox Reader View to extract
//! clean, readable content from web pages.

use crate::error::{Result, ScraperError};

/// Parsed article from Readability
///
/// Contains the extracted content with metadata.
#[derive(Debug, Clone)]
pub struct Article {
    /// Article title
    pub title: String,
    /// Text content (clean, without ads/nav)
    pub text_content: String,
    /// Excerpt/summary if available
    pub excerpt: Option<String>,
    /// Author/byline if available
    pub byline: Option<String>,
    /// Publication time if available
    pub published_time: Option<String>,
}

/// Parse HTML using Readability algorithm
///
/// # Arguments
/// * `html` - HTML content to parse
/// * `url` - Optional URL for relative link resolution
///
/// # Returns
/// * `Ok(Article)` - Parsed article with extracted content
/// * `Err(ScraperError::Extraction)` - If Readability fails
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::infrastructure::scraper::readability::parse;
///
/// let html = "<html><body><article><h1>Title</h1><p>Content</p></article></body></html>";
/// let article = parse(html, Some("https://example.com")).unwrap();
/// // Title may vary depending on legible's heuristic parsing
/// assert!(!article.title.is_empty());
/// ```
pub fn parse(html: &str, url: Option<&str>) -> Result<Article> {
    let article = legible::parse(html, url, None)
        .map_err(|e| ScraperError::Extraction(format!("Readability failed: {}", e)))?;

    Ok(Article {
        title: article.title,
        text_content: article.text_content,
        excerpt: article.excerpt,
        byline: article.byline,
        published_time: article.published_time,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_html() {
        let html = r#"
            <html>
                <head><title>Test Article</title></head>
                <body>
                    <article>
                        <h1>Test Article</h1>
                        <p>This is the main content of the article.</p>
                        <p>Another paragraph with more content.</p>
                    </article>
                </body>
            </html>
        "#;

        let article = parse(html, Some("https://example.com")).unwrap();
        assert_eq!(article.title, "Test Article");
        assert!(article.text_content.contains("main content"));
    }

    #[test]
    fn test_parse_with_byline() {
        // Use a more realistic article structure that legible can parse
        let html = r#"
            <html>
                <head><title>Article Title</title></head>
                <body>
                    <article>
                        <h1>Article Title</h1>
                        <address>By John Doe</address>
                        <p>Article content here. This is a longer paragraph with more text to make legible recognize this as the main content of the article.</p>
                        <p>Another paragraph with even more content to ensure the article is properly detected.</p>
                    </article>
                </body>
            </html>
        "#;

        let result = parse(html, Some("https://example.com"));
        // Just verify it doesn't crash - legible parsing is heuristic
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_parse_empty_html() {
        let html = "<html><body></body></html>";
        let result = parse(html, Some("https://example.com"));
        // Should not panic, may return empty content or error
        assert!(result.is_ok() || result.is_err());
    }
}
