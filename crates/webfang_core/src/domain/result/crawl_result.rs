//! Crawl result type
//!
//! Represents the outcome of a crawling operation.

use std::collections::BTreeMap;

use crate::domain::crawl_job::DiscoveredUrl;
use crate::domain::CrawlErrorCategory;

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
    /// Error counts by category (issue #374)
    pub error_breakdown: BTreeMap<CrawlErrorCategory, usize>,
}

impl CrawlResult {
    /// Create a new crawl result
    pub fn new(
        urls: Vec<DiscoveredUrl>,
        total_pages: usize,
        errors: usize,
        error_breakdown: BTreeMap<CrawlErrorCategory, usize>,
    ) -> Self {
        Self {
            urls,
            total_pages,
            errors,
            error_breakdown,
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
        assert!(result.error_breakdown.is_empty());
    }

    #[test]
    fn test_crawl_result_new() {
        let url = Url::parse("https://example.com").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::html(url, 0, parent);
        let result = CrawlResult::new(vec![discovered], 1, 0, BTreeMap::new());

        assert!(!result.is_empty());
        assert_eq!(result.total_pages, 1);
        assert_eq!(result.errors, 0);
        assert_eq!(result.urls.len(), 1);
        assert!(result.error_breakdown.is_empty());
    }

    #[test]
    fn test_crawl_result_with_breakdown() {
        let mut breakdown = BTreeMap::new();
        breakdown.insert(CrawlErrorCategory::Waf, 3);
        breakdown.insert(CrawlErrorCategory::Timeout, 2);
        let result = CrawlResult::new(vec![], 10, 5, breakdown);

        assert_eq!(result.errors, 5);
        assert_eq!(result.error_breakdown.len(), 2);
        assert_eq!(result.error_breakdown[&CrawlErrorCategory::Waf], 3);
        assert_eq!(result.error_breakdown[&CrawlErrorCategory::Timeout], 2);
    }
}
