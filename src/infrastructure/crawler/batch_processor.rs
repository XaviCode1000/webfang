//! Batch Processor Module
//!
//! Applies crawl budget optimization to URL collections.
//! Implements 80/20 rule: prioritize recent content (lastmod) and
//! filter parameter-heavy URLs to maximize crawl efficiency.

use crate::infrastructure::crawler::SitemapConfig;
use std::collections::HashSet;
use url::Url;

/// Errors that can occur during batch processing
#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    #[error("batch processing failed: {0}")]
    ProcessingFailed(String),
}

/// Result type for batch operations
pub type Result<T> = std::result::Result<T, BatchError>;

/// Handles batch processing with crawl budget optimization
pub struct BatchProcessor {
    max_params_threshold: usize,
}

impl BatchProcessor {
    /// Create new batch processor with default settings
    pub fn new() -> Self {
        Self {
            max_params_threshold: 5,
        }
    }

    /// Create with custom max params threshold
    pub fn with_max_params_threshold(max_params_threshold: usize) -> Self {
        Self {
            max_params_threshold,
        }
    }

    /// Apply crawl budget optimization to URL collection
    ///
    /// Applies the 80/20 rule by:
    /// 1. Prioritizing URLs with recent lastmod dates (if available in metadata)
    /// 2. Filtering out parameter-heavy URLs that waste crawl budget
    /// 3. Deduplicating similar URLs
    pub fn apply_crawl_budget(&self, urls: Vec<Url>, config: &SitemapConfig) -> Vec<Url> {
        if !config.crawl_budget_enabled {
            return urls;
        }

        // Step 1: Filter parameter-heavy URLs
        let filtered = self.filter_parameter_heavy_urls(urls);

        // Step 2: Deduplicate by normalized URL
        let deduplicated = self.deduplicate_urls(filtered);

        // Step 3: Sort by priority (would integrate with lastmod if available)
        // For now, just sort by path length as a proxy for importance
        let mut sorted = deduplicated;
        sorted.sort_by(|a, b| {
            let a_depth = a.path_segments().map(|s| s.count()).unwrap_or(0);
            let b_depth = b.path_segments().map(|s| s.count()).unwrap_or(0);
            b_depth.cmp(&a_depth) // Deeper paths first (more specific)
        });

        sorted
    }

    /// Filter URLs with excessive query parameters
    fn filter_parameter_heavy_urls(&self, urls: Vec<Url>) -> Vec<Url> {
        urls.into_iter()
            .filter(|url| {
                let param_count = url.query_pairs().count();
                param_count <= self.max_params_threshold
            })
            .collect()
    }

    /// Deduplicate URLs by normalizing them
    fn deduplicate_urls(&self, urls: Vec<Url>) -> Vec<Url> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut result = Vec::new();

        for url in urls {
            let normalized_str = url.to_string();

            if seen.insert(normalized_str) {
                result.push(url);
            }
        }

        result
    }
}

impl Default for BatchProcessor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_processor_creation() {
        let processor = BatchProcessor::new();
        // Just verify it can be created without panicking
        let _ = processor;
    }

    #[test]
    fn test_apply_crawl_budget_disabled() {
        let processor = BatchProcessor::new();
        let urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.com/page2").unwrap(),
        ];

        let config = SitemapConfig::default(); // crawl_budget_enabled = false
        let result = processor.apply_crawl_budget(urls.clone(), &config);

        assert_eq!(result.len(), urls.len());
    }

    #[test]
    fn test_apply_crawl_budget_filters_params() {
        let processor = BatchProcessor::new();
        let urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.com/page2?ref=abc").unwrap(),
            Url::parse("https://example.com/page3?a=1&b=2&c=3&d=4&e=5&f=6").unwrap(), // 6 params > threshold
        ];

        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        let result = processor.apply_crawl_budget(urls, &config);

        // Should filter out page3 with 6 params
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_apply_crawl_budget_deduplicates() {
        let processor = BatchProcessor::new();
        let urls = vec![
            Url::parse("https://example.com/page1").unwrap(),
            Url::parse("https://example.com/page1").unwrap(), // duplicate
            Url::parse("https://example.com/page2").unwrap(),
        ];

        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        let result = processor.apply_crawl_budget(urls, &config);

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_apply_crawl_budget_sorts_by_depth() {
        let processor = BatchProcessor::new();
        let urls = vec![
            Url::parse("https://example.com/a").unwrap(), // depth 1
            Url::parse("https://example.com/a/b").unwrap(), // depth 2
            Url::parse("https://example.com/a/b/c").unwrap(), // depth 3
        ];

        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        let result = processor.apply_crawl_budget(urls, &config);

        // Should be sorted by depth (deepest first)
        assert_eq!(result[0].path_segments().unwrap().count(), 3);
        assert_eq!(result[1].path_segments().unwrap().count(), 2);
        assert_eq!(result[2].path_segments().unwrap().count(), 1);
    }

    #[test]
    fn test_max_params_threshold_custom() {
        let processor = BatchProcessor::with_max_params_threshold(10);
        let urls = vec![
            Url::parse("https://example.com/page?a=1&b=2&c=3&d=4&e=5&f=6&g=7&h=8&i=9&j=10")
                .unwrap(), // 10 params
        ];

        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        let result = processor.apply_crawl_budget(urls, &config);

        // Should pass with threshold of 10
        assert_eq!(result.len(), 1);
    }
}
