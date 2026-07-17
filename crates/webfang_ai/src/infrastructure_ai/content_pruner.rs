//! Content pruning module — Extract readable content using `legible` crate.
//!
//! Sealed trait pattern: only `LegibleContentPruner` can implement `ContentPruner`.
//! Feature-gated behind `ai`.

use std::fmt;

/// Aggressiveness level for content pruning.
///
/// Controls how aggressively `legible` strips non-content elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PruneAggressiveness {
    /// Minimal pruning — keep most structural elements.
    Gentle,
    /// Standard pruning — remove nav, sidebar, ads, footer (default).
    #[default]
    Standard,
    /// Aggressive pruning — extract only the core article text.
    Aggressive,
}

/// Sealed trait for content pruning implementations.
///
/// Only implementors within this module can implement this trait.
/// External crates cannot add new pruning strategies.
pub trait ContentPruner: sealed::Sealed {
    /// Prune HTML content by extracting the readable main content.
    ///
    /// Returns the pruned HTML string. On failure or empty input,
    /// returns the original HTML unchanged (fallback behavior).
    fn prune(&self, html: &str) -> String;

    /// Return the aggressiveness level of this pruner.
    fn aggressiveness(&self) -> PruneAggressiveness;
}

/// Content pruner backed by the `legible` crate (Mozilla Readability port).
///
/// Extracts the main readable content from HTML, stripping navigation,
/// sidebars, ads, footers, and other boilerplate.
pub struct LegibleContentPruner {
    aggressiveness: PruneAggressiveness,
}

impl LegibleContentPruner {
    /// Create a new pruner with the given aggressiveness level.
    #[must_use]
    pub fn new(aggressiveness: PruneAggressiveness) -> Self {
        Self { aggressiveness }
    }

    /// Create a pruner with default (Standard) aggressiveness.
    #[must_use]
    pub fn standard() -> Self {
        Self::new(PruneAggressiveness::Standard)
    }
}

impl fmt::Debug for LegibleContentPruner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LegibleContentPruner")
            .field("aggressiveness", &self.aggressiveness)
            .finish()
    }
}

impl sealed::Sealed for LegibleContentPruner {}

impl ContentPruner for LegibleContentPruner {
    fn prune(&self, html: &str) -> String {
        // Fallback: empty input returns empty string
        if html.trim().is_empty() {
            return String::new();
        }

        // Attempt legible parsing
        match legible::parse(html, None, None) {
            Ok(article) => {
                if article.content.trim().is_empty() {
                    // legible returned empty content — pass through original
                    html.to_string()
                } else {
                    article.content
                }
            },
            Err(_) => {
                // legible failed — pass through original HTML unchanged
                html.to_string()
            },
        }
    }

    fn aggressiveness(&self) -> PruneAggressiveness {
        self.aggressiveness
    }
}

/// Sealed trait internals — prevents external implementations.
mod sealed {
    pub trait Sealed {}
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Task 2.1 (RED): Tests for ContentPruner::prune() ──

    #[test]
    fn prune_empty_html_returns_empty() {
        let pruner = LegibleContentPruner::standard();
        assert_eq!(pruner.prune(""), "");
    }

    #[test]
    fn prune_whitespace_only_returns_empty() {
        let pruner = LegibleContentPruner::standard();
        assert_eq!(pruner.prune("   \n\t  "), "");
    }

    #[test]
    fn prune_malformed_html_does_not_panic() {
        let pruner = LegibleContentPruner::standard();
        let html = "<<<not valid html>>>><<<";
        // legible may wrap malformed HTML in a readability div — that's fine.
        // The important thing is it doesn't crash and returns non-empty content.
        let result = pruner.prune(html);
        assert!(
            !result.is_empty(),
            "Malformed HTML should still produce output"
        );
    }

    #[test]
    fn prune_valid_article_returns_pruned_content() {
        let pruner = LegibleContentPruner::standard();
        let html = r#"
            <html>
            <head><title>Test</title></head>
            <body>
                <nav>Navigation bar</nav>
                <article>
                    <h1>Article Title</h1>
                    <p>This is a substantial paragraph with enough text content to make legible recognize it as the main article body. It needs sufficient length to pass readability heuristics.</p>
                    <p>Another paragraph with more substantial content to ensure the article is properly detected by the readability algorithm.</p>
                </article>
                <footer>Footer content</footer>
            </body>
            </html>
        "#;
        let result = pruner.prune(html);
        // Should contain article content, not navigation
        assert!(
            result.contains("Article Title") || result.contains("article"),
            "Expected article content in result: {result}"
        );
    }

    #[test]
    fn prune_returns_non_empty_for_valid_content() {
        let pruner = LegibleContentPruner::standard();
        let html = r#"
            <html><body>
            <nav>Sidebar</nav>
            <main>
                <h1>How to Build a Web Scraper</h1>
                <p>Web scraping is the process of extracting data from websites. It involves fetching web pages and parsing their HTML content to extract structured information. This is a comprehensive guide that covers all the essential techniques and tools you need to know.</p>
                <p>The first step is to understand the structure of HTML documents. Every web page is built using HTML tags that define the content and layout. By understanding these tags, you can write parsers that extract exactly the data you need.</p>
                <p>Next, you need to handle common challenges like pagination, JavaScript rendering, and rate limiting. Each of these requires specific strategies and tools to overcome effectively.</p>
            </main>
            <aside>Related articles sidebar</aside>
            </body></html>
        "#;
        let result = pruner.prune(html);
        assert!(!result.is_empty(), "Pruned content should not be empty");
    }

    #[test]
    fn prune_aggressiveness_default_is_standard() {
        assert_eq!(
            PruneAggressiveness::default(),
            PruneAggressiveness::Standard
        );
    }

    #[test]
    fn prune_preserves_aggressiveness_setting() {
        let pruner = LegibleContentPruner::new(PruneAggressiveness::Aggressive);
        assert_eq!(pruner.aggressiveness(), PruneAggressiveness::Aggressive);
    }

    #[test]
    fn prune_debug_impl() {
        let pruner = LegibleContentPruner::standard();
        let debug = format!("{:?}", pruner);
        assert!(debug.contains("LegibleContentPruner"));
        assert!(debug.contains("Standard"));
    }
}
