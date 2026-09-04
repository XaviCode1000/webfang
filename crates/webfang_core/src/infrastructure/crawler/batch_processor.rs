//! Batch Processor Module
//!
//! Applies crawl budget optimization to URL collections.
//! Implements 80/20 rule: prioritize recent content (lastmod) and
//! filter parameter-heavy URLs to maximize crawl efficiency.

use crate::domain::crawler_port::SitemapConfig;
use crate::domain::url_validation::{NormalizeConfig, RemoveQueryParameters};
use crate::infrastructure::crawler::{normalize_url, SitemapUrl};
use std::collections::HashSet;
#[cfg(test)]
use url::Url;

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
    pub fn apply_crawl_budget(
        &self,
        urls: Vec<SitemapUrl>,
        config: &SitemapConfig,
    ) -> Vec<SitemapUrl> {
        if !config.crawl_budget_enabled {
            return urls;
        }

        // Step 1: Filter parameter-heavy URLs
        let filtered = self.filter_parameter_heavy_urls(urls);

        // Step 2: Deduplicate by normalized URL
        let deduplicated = self.deduplicate_urls(filtered);

        // Step 3: Sort by priority desc, then lastmod desc (deterministic ordering)
        let mut sorted = deduplicated;
        sorted.sort_by(|a, b| {
            let pri_a = a.priority.unwrap_or(0.5);
            let pri_b = b.priority.unwrap_or(0.5);
            pri_b
                .partial_cmp(&pri_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.lastmod.cmp(&a.lastmod))
        });

        sorted
    }

    /// Filter URLs with excessive query parameters
    fn filter_parameter_heavy_urls(&self, urls: Vec<SitemapUrl>) -> Vec<SitemapUrl> {
        urls.into_iter()
            .filter(|url| {
                let param_count = url.url.query_pairs().count();
                param_count <= self.max_params_threshold
            })
            .collect()
    }

    /// Deduplicate URLs by normalizing them
    fn deduplicate_urls(&self, urls: Vec<SitemapUrl>) -> Vec<SitemapUrl> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut result = Vec::new();

        for url in urls {
            let normalized_str = normalize_url(
                url.url.as_str(),
                &NormalizeConfig {
                    strip_www: true,
                    query_policy: RemoveQueryParameters::All,
                },
            );

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
    use crate::infrastructure::crawler::SitemapUrl;

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
            SitemapUrl::new(Url::parse("https://example.com/page1").unwrap()),
            SitemapUrl::new(Url::parse("https://example.com/page2").unwrap()),
        ];

        let config = SitemapConfig::default(); // crawl_budget_enabled = false
        let result = processor.apply_crawl_budget(urls.clone(), &config);

        assert_eq!(result.len(), urls.len());
    }

    #[test]
    fn test_apply_crawl_budget_filters_params() {
        let processor = BatchProcessor::new();
        let urls = vec![
            SitemapUrl::new(Url::parse("https://example.com/page1").unwrap()),
            SitemapUrl::new(Url::parse("https://example.com/page2?ref=abc").unwrap()),
            SitemapUrl::new(
                Url::parse("https://example.com/page3?a=1&b=2&c=3&d=4&e=5&f=6").unwrap(),
            ), // 6 params > threshold
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
            SitemapUrl::new(Url::parse("https://example.com/page1").unwrap()),
            SitemapUrl::new(Url::parse("https://example.com/page1").unwrap()), // duplicate
            SitemapUrl::new(Url::parse("https://example.com/page2").unwrap()),
        ];

        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        let result = processor.apply_crawl_budget(urls, &config);

        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_apply_crawl_budget_sorts_by_priority_and_lastmod() {
        let processor = BatchProcessor::new();
        let urls = vec![
            SitemapUrl {
                url: Url::parse("https://example.com/a").unwrap(),
                lastmod: Some("2024-01-01".to_string()),
                priority: Some(0.5),
                changefreq: None,
            },
            SitemapUrl {
                url: Url::parse("https://example.com/b").unwrap(),
                lastmod: Some("2024-01-02".to_string()),
                priority: Some(0.8),
                changefreq: None,
            },
            SitemapUrl {
                url: Url::parse("https://example.com/c").unwrap(),
                lastmod: Some("2024-01-03".to_string()),
                priority: Some(0.8),
                changefreq: None,
            },
        ];

        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        let result = processor.apply_crawl_budget(urls, &config);

        // Should be sorted by priority desc, then lastmod desc
        // b and c have priority 0.8, c has newer lastmod so comes first
        // a has priority 0.5 so comes last
        assert_eq!(result[0].url.as_str(), "https://example.com/c");
        assert_eq!(result[1].url.as_str(), "https://example.com/b");
        assert_eq!(result[2].url.as_str(), "https://example.com/a");
    }

    #[test]
    fn test_max_params_threshold_custom() {
        let processor = BatchProcessor::with_max_params_threshold(10);
        let urls = vec![
            SitemapUrl::new(
                Url::parse("https://example.com/page?a=1&b=2&c=3&d=4&e=5&f=6&g=7&h=8&i=9&j=10")
                    .unwrap(),
            ), // 10 params
        ];

        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        let result = processor.apply_crawl_budget(urls, &config);

        // Should pass with threshold of 10
        assert_eq!(result.len(), 1);
    }
}
