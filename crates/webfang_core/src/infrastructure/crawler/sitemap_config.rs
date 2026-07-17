//! Sitemap configuration with builder pattern
//!
//! Following api-builder-pattern: clear, self-documenting API

use crate::domain::CompressionType;

/// Sitemap parser configuration
///
/// Following api-builder-pattern: clear, self-documenting API
#[derive(Debug, Clone)]
pub struct SitemapConfig {
    /// Enable gzip decompression (default: true)
    pub gzip_enabled: bool,
    /// Maximum recursion depth for sitemap indexes (default: 3)
    pub max_depth: u8,
    /// Concurrent requests for sitemap indexes (default: 5)
    pub concurrency: usize,
    /// Maximum HTTP response size in bytes (default: 50MB)
    pub max_response_size: usize,
    /// Maximum decompressed gzip size in bytes (default: 100MB)
    pub max_decompressed_size: usize,
    /// Enable pagination for large sitemaps (default: false)
    pub pagination_enabled: bool,
    /// Batch size for pagination (default: 10,000)
    pub batch_size: usize,
    /// Supported compression types (default: [Gzip])
    pub compression_types: Vec<CompressionType>,
    /// Enable URL validation and filtering (default: false)
    pub url_validation_enabled: bool,
    /// Memory limit in MB for processing (default: 500)
    pub memory_limit_mb: usize,
    /// Enable crawl budget optimization (default: false)
    pub crawl_budget_enabled: bool,
}

impl Default for SitemapConfig {
    fn default() -> Self {
        Self {
            gzip_enabled: true,
            max_depth: 3,
            concurrency: 5,
            max_response_size: 52_428_800,      // 50MB
            max_decompressed_size: 104_857_600, // 100MB
            pagination_enabled: false,
            batch_size: 10_000,
            compression_types: vec![CompressionType::Gzip],
            url_validation_enabled: false,
            memory_limit_mb: 500,
            crawl_budget_enabled: false,
        }
    }
}

impl SitemapConfig {
    /// Create new config builder
    pub fn builder() -> SitemapConfigBuilder {
        SitemapConfigBuilder::default()
    }
}

/// Builder for SitemapConfig
///
/// Following api-builder-must-use: #[must_use] attribute
#[derive(Default)]
#[must_use = "builders do nothing unless you call build()"]
pub struct SitemapConfigBuilder {
    gzip_enabled: bool,
    max_depth: u8,
    concurrency: usize,
    max_response_size: usize,
    max_decompressed_size: usize,
    pagination_enabled: bool,
    batch_size: usize,
    compression_types: Vec<CompressionType>,
    url_validation_enabled: bool,
    memory_limit_mb: usize,
    crawl_budget_enabled: bool,
}

impl SitemapConfigBuilder {
    /// Enable or disable gzip decompression
    pub fn gzip_enabled(mut self, enabled: bool) -> Self {
        self.gzip_enabled = enabled;
        self
    }

    /// Set maximum recursion depth for sitemap indexes
    pub fn max_depth(mut self, depth: u8) -> Self {
        self.max_depth = depth;
        self
    }

    /// Set concurrency level for parallel sitemap parsing
    pub fn concurrency(mut self, count: usize) -> Self {
        self.concurrency = count;
        self
    }

    /// Set maximum HTTP response size in bytes
    pub fn max_response_size(mut self, size: usize) -> Self {
        self.max_response_size = size;
        self
    }

    /// Set maximum decompressed gzip size in bytes
    pub fn max_decompressed_size(mut self, size: usize) -> Self {
        self.max_decompressed_size = size;
        self
    }

    /// Enable or disable pagination for large sitemaps
    pub fn pagination_enabled(mut self, enabled: bool) -> Self {
        self.pagination_enabled = enabled;
        self
    }

    /// Set batch size for pagination
    pub fn batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set supported compression types
    pub fn compression_types(mut self, types: Vec<CompressionType>) -> Self {
        self.compression_types = types;
        self
    }

    /// Enable or disable URL validation and filtering
    pub fn url_validation_enabled(mut self, enabled: bool) -> Self {
        self.url_validation_enabled = enabled;
        self
    }

    /// Set memory limit in MB for processing
    pub fn memory_limit_mb(mut self, mb: usize) -> Self {
        self.memory_limit_mb = mb;
        self
    }

    /// Enable or disable crawl budget optimization
    pub fn crawl_budget_enabled(mut self, enabled: bool) -> Self {
        self.crawl_budget_enabled = enabled;
        self
    }

    /// Build the configuration
    #[must_use]
    pub fn build(self) -> SitemapConfig {
        let defaults = SitemapConfig::default();
        SitemapConfig {
            gzip_enabled: self.gzip_enabled,
            max_depth: self.max_depth,
            concurrency: self.concurrency,
            max_response_size: if self.max_response_size == 0 {
                defaults.max_response_size
            } else {
                self.max_response_size
            },
            max_decompressed_size: if self.max_decompressed_size == 0 {
                defaults.max_decompressed_size
            } else {
                self.max_decompressed_size
            },
            pagination_enabled: self.pagination_enabled,
            batch_size: if self.batch_size == 0 {
                defaults.batch_size
            } else {
                self.batch_size
            },
            compression_types: if self.compression_types.is_empty() {
                defaults.compression_types
            } else {
                self.compression_types
            },
            url_validation_enabled: self.url_validation_enabled,
            memory_limit_mb: if self.memory_limit_mb == 0 {
                defaults.memory_limit_mb
            } else {
                self.memory_limit_mb
            },
            crawl_budget_enabled: self.crawl_budget_enabled,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SitemapConfig::default();
        assert!(config.gzip_enabled);
        assert_eq!(config.max_depth, 3);
        assert_eq!(config.concurrency, 5);
        assert_eq!(config.max_response_size, 52_428_800);
        assert_eq!(config.max_decompressed_size, 104_857_600);
    }

    #[test]
    fn test_pagination_enabled_builder_false() {
        let config = SitemapConfig::builder().pagination_enabled(false).build();

        assert!(!config.pagination_enabled);
    }

    #[test]
    fn test_batch_size_builder() {
        let config = SitemapConfig::builder().batch_size(5_000).build();

        assert_eq!(config.batch_size, 5_000);
    }

    #[test]
    fn test_batch_size_zero_fallback() {
        let config = SitemapConfig::builder().batch_size(0).build();

        assert_eq!(config.batch_size, 10_000);
    }

    #[test]
    fn test_compression_types_default() {
        let config = SitemapConfig::default();
        assert_eq!(config.compression_types, vec![CompressionType::Gzip]);
    }

    #[test]
    fn test_compression_types_empty_fallback() {
        use crate::domain::CompressionType;

        let config = SitemapConfig::builder().compression_types(vec![]).build();

        assert_eq!(config.compression_types, vec![CompressionType::Gzip]);
    }

    #[test]
    fn test_url_validation_enabled_default() {
        let config = SitemapConfig::default();
        assert!(!config.url_validation_enabled);
    }

    #[test]
    fn test_memory_limit_mb_zero_fallback() {
        let config = SitemapConfig::builder().memory_limit_mb(0).build();

        assert_eq!(config.memory_limit_mb, 500);
    }

    #[test]
    fn test_crawl_budget_enabled_builder() {
        let config = SitemapConfig::builder().crawl_budget_enabled(true).build();

        assert!(config.crawl_budget_enabled);
    }

    #[test]
    fn test_crawl_budget_enabled_builder_false() {
        let config = SitemapConfig::builder().crawl_budget_enabled(false).build();

        assert!(!config.crawl_budget_enabled);
    }

    #[test]
    fn test_url_validation_enabled_builder_false() {
        let config = SitemapConfig::builder()
            .url_validation_enabled(false)
            .build();

        assert!(!config.url_validation_enabled);
    }

    #[test]
    fn test_builder_zero_values_use_defaults() {
        let config = SitemapConfig::builder()
            .max_response_size(0)
            .max_decompressed_size(0)
            .build();

        assert_eq!(config.max_response_size, 52_428_800);
        assert_eq!(config.max_decompressed_size, 104_857_600);
    }
}
