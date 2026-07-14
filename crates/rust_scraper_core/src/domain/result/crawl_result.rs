//! Crawl result type
//!
//! Represents the outcome of a crawling operation.

use crate::domain::crawl_job::DiscoveredUrl;

/// Crawl result containing discovered URLs
///
/// Following **api-must-use** and **api-non-exhaustive**.
#[derive(Debug, Clone, Default)]
#[must_use]
#[non_exhaustive]
pub struct CrawlResult {
    /// All discovered URLs
    pub urls: Vec<DiscoveredUrl>,
    /// Total number of pages crawled
    pub total_pages: usize,
    /// Number of errors encountered
    pub errors: usize,
}

impl CrawlResult {
    /// Create a new crawl result
    pub fn new(urls: Vec<DiscoveredUrl>, total_pages: usize, errors: usize) -> Self {
        Self {
            urls,
            total_pages,
            errors,
        }
    }

    /// Create an empty crawl result
    pub fn empty() -> Self {
        Self::default()
    }

    /// Check if the result is empty
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.urls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use url::Url;

    #[test]
    fn test_crawl_result_empty() {
        let result = CrawlResult::empty();
        assert!(result.is_empty());
        assert_eq!(result.total_pages, 0);
        assert_eq!(result.errors, 0);
    }

    #[test]
    fn test_crawl_result_new() {
        let url = Url::parse("https://example.com").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::html(url, 0, parent);
        let result = CrawlResult::new(vec![discovered], 1, 0);

        assert!(!result.is_empty());
        assert_eq!(result.total_pages, 1);
        assert_eq!(result.errors, 0);
        assert_eq!(result.urls.len(), 1);
    }
}
