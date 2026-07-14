//! Reusable test data generators and helpers.
//!
//! These fixtures provide deterministic, version-controlled test data that
//! avoids network access and external dependencies. They are consumed by
//! integration tests across all workspace crates.

// Shared fixture module is included as a submodule in multiple integration-test
// crates. An item used by one test crate is reported as dead_code in the others,
// so the lint is suppressed here at module scope.
#![allow(dead_code)]

use std::path::PathBuf;
use tempfile::TempDir;

// ============================================================================
// HTML Fixtures
// ============================================================================

/// Sample HTML page with title, content, and links.
///
/// Useful for testing HTML parsing, content extraction, and link discovery.
pub fn sample_html() -> &'static str {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <title>Test Article - Example Domain</title>
    <meta name="description" content="A test article for parsing">
</head>
<body>
    <article>
        <h1>Test Article</h1>
        <p class="author">By Test Author</p>
        <time datetime="2024-01-15">January 15, 2024</time>
        <div class="content">
            <p>This is the main content of the article. It contains
            multiple paragraphs for testing text extraction.</p>
            <p>Second paragraph with <a href="https://example.com/linked">a link</a>
            and <strong>bold text</strong>.</p>
        </div>
        <img src="/images/test.jpg" alt="Test image">
        <a href="/page/2">Next page</a>
        <a href="https://external.com/page">External link</a>
    </article>
</body>
</html>"#
}

/// Minimal valid HTML for quick parsing tests.
pub fn sample_minimal_html() -> &'static str {
    r#"<!DOCTYPE html>
<html>
<head><title>Minimal</title></head>
<body><p>Hello world</p></body>
</html>"#
}

/// HTML with common scraping challenges (scripts, nav, ads).
pub fn sample_noisy_html() -> &'static str {
    r#"<!DOCTYPE html>
<html>
<head>
    <title>Noisy Page</title>
    <script>var tracking = true;</script>
    <style>.hidden { display: none; }</style>
</head>
<body>
    <nav><a href="/">Home</a> <a href="/about">About</a></nav>
    <div class="ad-banner">ADVERTISEMENT</div>
    <main>
        <h1>Actual Content</h1>
        <p>The real content is here, buried among noise.</p>
    </main>
    <footer>Copyright 2024</footer>
    <script>console.log("analytics");</script>
</body>
</html>"#
}

/// HTML with nested structures for depth testing.
pub fn sample_nested_html() -> &'static str {
    r#"<!DOCTYPE html>
<html>
<head><title>Nested</title></head>
<body>
    <div class="level-1">
        <div class="level-2">
            <div class="level-3">
                <p>Deep content</p>
            </div>
        </div>
    </div>
</body>
</html>"#
}

// ============================================================================
// Sitemap Fixtures
// ============================================================================

/// Standard sitemap XML with multiple URLs.
pub fn sample_sitemap() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url>
        <loc>https://example.com/</loc>
        <lastmod>2024-01-15</lastmod>
        <changefreq>daily</changefreq>
        <priority>1.0</priority>
    </url>
    <url>
        <loc>https://example.com/about</loc>
        <lastmod>2024-01-10</lastmod>
        <changefreq>monthly</changefreq>
        <priority>0.8</priority>
    </url>
    <url>
        <loc>https://example.com/contact</loc>
        <changefreq>yearly</changefreq>
        <priority>0.5</priority>
    </url>
</urlset>"#
}

/// Sitemap index referencing child sitemaps.
pub fn sample_sitemap_index() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <sitemap>
        <loc>https://example.com/sitemap-posts.xml</loc>
        <lastmod>2024-01-15</lastmod>
    </sitemap>
    <sitemap>
        <loc>https://example.com/sitemap-pages.xml</loc>
        <lastmod>2024-01-10</lastmod>
    </sitemap>
</sitemapindex>"#
}

/// Empty sitemap (no URLs).
pub fn sample_empty_sitemap() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#
}

/// Sitemap with special characters in URLs.
pub fn sample_sitemap_special_chars() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url>
        <loc>https://example.com/path%20with%20spaces</loc>
    </url>
    <url>
        <loc>https://example.com/search?q=rust&amp;lang=en</loc>
    </url>
    <url>
        <loc>https://example.com/unicode/café</loc>
    </url>
</urlset>"#
}

// ============================================================================
// JSON Fixtures
// ============================================================================

/// Sample scraped content as JSON.
pub fn sample_scraped_content_json() -> &'static str {
    r#"{
    "title": "Test Article",
    "content": "Test content here.",
    "url": "https://example.com/test",
    "author": "Test Author",
    "date": "2024-01-15"
}"#
}

/// Sample error response JSON.
pub fn sample_error_json() -> &'static str {
    r#"{
    "error": {
        "code": 404,
        "message": "Not Found",
        "details": "The requested resource does not exist"
    }
}"#
}

// ============================================================================
// Temporary Directory Helper
// ============================================================================

/// RAII wrapper for a temporary directory.
///
/// Automatically cleans up when dropped. Use `path()` to get the directory path.
///
/// # Example
///
/// ```ignore
/// let tmp = TempDirHelper::new();
/// let file_path = tmp.path().join("output.json");
/// std::fs::write(&file_path, "{}").unwrap();
/// // Directory is cleaned up when `tmp` is dropped
/// ```
pub struct TempDirHelper {
    _inner: TempDir,
    path: PathBuf,
}

impl TempDirHelper {
    /// Create a new temporary directory.
    pub fn new() -> Self {
        let inner = TempDir::new().expect("failed to create temp dir");
        let path = inner.path().to_path_buf();
        Self {
            _inner: inner,
            path,
        }
    }

    /// Get the path to the temporary directory.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Default for TempDirHelper {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// URL Helpers
// ============================================================================

/// Generate a list of test URLs.
pub fn sample_urls(count: usize) -> Vec<String> {
    (0..count)
        .map(|i| format!("https://example.com/page/{i}"))
        .collect()
}

/// A valid base URL for testing.
pub const TEST_BASE_URL: &str = "https://example.com";

// ============================================================================
// Unit Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_html_has_title() {
        assert!(sample_html().contains("<title>Test Article"));
    }

    #[test]
    fn test_sample_html_has_content() {
        assert!(sample_html().contains("main content"));
    }

    #[test]
    fn test_sample_html_has_links() {
        assert!(sample_html().contains("href="));
    }

    #[test]
    fn test_sample_sitemap_is_valid_xml() {
        let sitemap = sample_sitemap();
        assert!(sitemap.contains("<urlset"));
        assert!(sitemap.contains("<loc>"));
    }

    #[test]
    fn test_sample_sitemap_index_is_valid() {
        let index = sample_sitemap_index();
        assert!(index.contains("<sitemapindex"));
    }

    #[test]
    fn test_sample_empty_sitemap() {
        let empty = sample_empty_sitemap();
        assert!(empty.contains("<urlset"));
        // No <url> elements
        assert!(!empty.contains("<url>"));
    }

    #[test]
    fn test_sample_urls_generates_correct_count() {
        let urls = sample_urls(5);
        assert_eq!(urls.len(), 5);
        assert_eq!(urls[0], "https://example.com/page/0");
        assert_eq!(urls[4], "https://example.com/page/4");
    }

    #[test]
    fn test_temp_dir_helper_creates_directory() {
        let tmp = TempDirHelper::new();
        assert!(tmp.path().exists());
        assert!(tmp.path().is_dir());
    }

    #[test]
    fn test_minimal_html() {
        assert!(sample_minimal_html().contains("Hello world"));
    }

    #[test]
    fn test_noisy_html_has_content() {
        assert!(sample_noisy_html().contains("Actual Content"));
    }
}
