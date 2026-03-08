//! YAML frontmatter generation
//!
//! Generates YAML frontmatter for Markdown files with metadata:
//! - Title
//! - URL
//! - Date (publication or scrape date)
//! - Author (if available)
//! - Excerpt (if available)

use chrono::Utc;
use serde::Serialize;

/// Frontmatter data structure
#[derive(Debug, Serialize)]
struct Frontmatter {
    /// Article/page title
    title: String,
    /// Original URL
    url: String,
    /// Publication/scrape date (YYYY-MM-DD)
    date: String,
    /// Author name (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    /// Excerpt/summary (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
}

/// Generate YAML frontmatter for a markdown file
///
/// # Arguments
/// * `title` - Article/page title
/// * `url` - Original URL
/// * `date` - Publication date (optional, uses current date if None)
/// * `author` - Author name (optional)
/// * `excerpt` - Excerpt/summary (optional)
///
/// # Returns
/// YAML string without the surrounding `---` delimiters
///
/// # Examples
///
/// ```
/// use rust_scraper::infrastructure::output::frontmatter::generate;
///
/// let fm = generate(
///     "My Article",
///     "https://example.com/article",
///     Some("2024-01-15"),
///     Some("John Doe"),
///     Some("A short excerpt"),
/// );
/// assert!(fm.contains("title: My Article"));
/// assert!(fm.contains("url: https://example.com/article"));
/// ```
pub fn generate(
    title: &str,
    url: &str,
    date: Option<&str>,
    author: Option<&str>,
    excerpt: Option<&str>,
) -> String {
    let fm = Frontmatter {
        title: title.to_string(),
        url: url.to_string(),
        date: date
            .map(|s| s.to_string())
            .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string()),
        author: author.map(|s| s.to_string()),
        excerpt: excerpt.map(|s| s.to_string()),
    };

    serde_yaml::to_string(&fm).unwrap_or_else(|_| String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_with_all_fields() {
        let fm = generate(
            "Test Title",
            "https://example.com",
            Some("2024-01-15"),
            Some("John Doe"),
            Some("Test excerpt"),
        );

        assert!(fm.contains("title: Test Title"));
        assert!(fm.contains("url: https://example.com"));
        assert!(fm.contains("date:")); // Date format may vary
        assert!(fm.contains("author: John Doe"));
        assert!(fm.contains("excerpt: Test excerpt"));
    }

    #[test]
    fn test_generate_with_auto_date() {
        let fm = generate("Test", "https://example.com", None, None, None);

        assert!(fm.contains("title: Test"));
        assert!(fm.contains("url: https://example.com"));
        // Date should be today (format may vary)
        assert!(fm.contains("date:"));
        assert!(!fm.contains("author"));
        assert!(!fm.contains("excerpt"));
    }

    #[test]
    fn test_generate_minimal() {
        let fm = generate("Minimal", "https://minimal.com", None, None, None);

        assert!(fm.contains("title: Minimal"));
        assert!(fm.contains("url: https://minimal.com"));
    }
}
