//! Link extraction domain interface
//!
//! Defines the contract for extracting links from HTML content.
//! Infrastructure layer implements this trait.

use crate::domain::CrawlError;

/// Domain interface for link extraction
///
/// This trait defines the contract for extracting and normalizing
/// links from HTML content. The infrastructure layer provides
/// the implementation using external libraries like scraper.
///
/// `Send + Sync` because crawl tasks poll extractors from
/// `tokio::spawn`-ed tasks on the multi-threaded runtime (the
/// application-layer port that wraps this trait requires it).
pub trait LinkExtractor: Send + Sync {
    /// Extract all links from HTML content
    ///
    /// # Arguments
    ///
    /// * `html` - HTML content to parse
    /// * `base_url` - Base URL for resolving relative links
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<String>)` - List of extracted, normalized URLs
    /// * `Err(CrawlError)` - Parse or processing error
    fn extract_links(&self, html: &str, base_url: &str) -> Result<Vec<String>, CrawlError>;
}

/// Domain service for link processing logic
///
/// Contains pure functions for link normalization and validation
/// that don't depend on external libraries.
pub struct LinkProcessor;

impl LinkProcessor {
    /// Check if a URL is internal (same domain)
    ///
    /// Delegates to the canonical, port-safe
    /// [`crate::domain::url_validation::is_internal_link`].
    pub fn is_internal_link(url: &str, domain: &str) -> bool {
        crate::domain::url_validation::is_internal_link(url, domain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_processor_is_internal_link() {
        assert!(LinkProcessor::is_internal_link(
            "https://example.com/page",
            "example.com"
        ));
        assert!(LinkProcessor::is_internal_link(
            "https://www.example.com/page",
            "example.com"
        ));
        assert!(LinkProcessor::is_internal_link(
            "https://blog.example.com/post",
            "example.com"
        ));
        assert!(!LinkProcessor::is_internal_link(
            "https://other.com/page",
            "example.com"
        ));
        assert!(!LinkProcessor::is_internal_link(
            "invalid-url",
            "example.com"
        ));
    }
}
