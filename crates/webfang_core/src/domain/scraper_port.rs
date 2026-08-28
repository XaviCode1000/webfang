//! Scraper port — domain-owned trait for HTML extraction.

use crate::domain::dom_inspector::ExtractResult;
use crate::error::ScraperError;

/// Fallback text extraction (domain pure, htmd).
pub mod fallback {
    /// Extract text without Readability (basic HTML stripping).
    #[must_use]
    pub fn extract_text(html: &str) -> String {
        htmd::convert(html).unwrap_or_else(|_| {
            html.lines()
                .map(|line| line.trim())
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
    }
}

/// Readability wrapper (domain pure, legible).
pub mod readability {
    use crate::error::{Result, ScraperError};

    /// Parsed article from Readability.
    #[derive(Debug, Clone)]
    pub struct Article {
        /// Article title.
        pub title: String,
        /// Clean HTML content.
        pub content: String,
        /// Text content.
        pub text_content: String,
        /// Excerpt/summary if available.
        pub excerpt: Option<String>,
        /// Author/byline if available.
        pub byline: Option<String>,
        /// Publication time if available.
        pub published_time: Option<String>,
    }

    /// Parse HTML using Readability algorithm.
    pub fn parse(html: &str, url: Option<&str>) -> Result<Article> {
        let article = legible::parse(html, url, None)
            .map_err(|e| ScraperError::Extraction(format!("Readability failed: {e}")))?;
        Ok(Article {
            title: article.title,
            content: article.content,
            text_content: article.text_content,
            excerpt: article.excerpt,
            byline: article.byline,
            published_time: article.published_time,
        })
    }
}

/// Domain port for HTML extraction.
///
/// Two strategies:
/// - `fallback` — basic `htmd` conversion (infallible).
/// - `readability` — Firefox Reader View (may fail).
pub trait ScraperPort: Send + Sync {
    /// Extract text without Readability (basic HTML stripping).
    fn fallback(&self, html: &str) -> ExtractResult;

    /// Parse HTML using Readability algorithm.
    ///
    /// # Errors
    ///
    /// Returns `ScraperError` when parsing fails.
    fn readability(&self, html: &str) -> Result<ExtractResult, ScraperError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::dom_inspector::ExtractResult;

    struct FakeScraper;

    impl ScraperPort for FakeScraper {
        fn fallback(&self, html: &str) -> ExtractResult {
            if html.contains("match") {
                ExtractResult::Matched("extracted".into())
            } else {
                ExtractResult::Fallback {
                    html: html.to_string(),
                    diagnostic: None,
                }
            }
        }

        fn readability(&self, html: &str) -> Result<ExtractResult, ScraperError> {
            if html.is_empty() {
                return Err(ScraperError::Extraction("empty html".into()));
            }
            Ok(ExtractResult::Matched(format!("read:{html}")))
        }
    }

    #[test]
    fn fallback_returns_matched_or_fallback() {
        let s = FakeScraper;
        let m = s.fallback("match this");
        assert!(m.is_matched());
        assert_eq!(m.as_html(), "extracted");

        let f = s.fallback("nope");
        assert!(!f.is_matched());
        assert_eq!(f.as_html(), "nope");
    }

    #[test]
    fn readability_ok_and_err() {
        let s = FakeScraper;
        let ok = s.readability("<p>hi</p>").expect("should succeed");
        assert!(ok.is_matched());
        assert!(ok.as_html().contains("read:"));

        let err = s.readability("").expect_err("empty must fail");
        assert!(err.to_string().contains("empty"));

        // Second ok with different html triangulates.
        let ok2 = s.readability("<div>other</div>").unwrap();
        assert!(ok2.as_html().contains("other"));
    }

    #[test]
    fn scraper_port_is_object_safe() {
        fn assert_dyn(_: &dyn ScraperPort) {}
        assert_dyn(&FakeScraper);
    }
}
