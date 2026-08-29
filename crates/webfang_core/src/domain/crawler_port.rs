//! Crawler port — domain-owned SitemapConfig and helper surface.
//!
//! Extracted from `infrastructure::crawler::sitemap_config` and helpers
//! (`extract_links`, `is_internal_link`, `normalize_url`, `derive_filename`)
//! so `application::crawler::*` can depend on `domain::*` without
//! `application→infrastructure` (ADR-0010). Infrastructure keeps a `pub use`
//! shim and the `LinkExtractor` impl.
//!
//! # Sub-slice 3.A (ADR-0012)
//!
//! Moves the `UrlSource` enum from `infrastructure::crawler::url_queue`
//! into domain. The remaining `infrastructure::crawler` types
//! (`UrlQueue`, `RobotsFetcher`, `SitemapParser`, `FsBinaryWriter`,
//! `extract_links`, `parse_sitemap`, `binary_utils::*`) are
//! infrastructure concerns and will be addressed in dedicated
//! sub-slices — `RobotsFetcher` and `SitemapParser` need domain
//! traits (3.C, 3.A-trait), the rest are pure re-exports to be
//! bundled in 3.A+ (DTO migration).

use crate::domain::CompressionType;

// Re-export pure helpers that already live in domain (inward-only surface).
pub use crate::domain::link_extractor::{LinkExtractor, LinkProcessor};
pub use crate::domain::url_validation::{
    canonical_path, extract_domain, is_internal_link, normalize_seed_host, normalize_url,
    NormalizeConfig,
};
pub use url_normalize::RemoveQueryParameters;

// ============================================================================
// UrlSource — moved from infrastructure::crawler::url_queue in sub-slice 3.A
// ============================================================================

/// Source of a discovered URL, used for priority scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlSource {
    /// Seed URL (the initial URL to crawl)
    Seed,
    /// URL discovered from a sitemap
    Sitemap,
    /// URL discovered from page links
    Link,
}

// ============================================================================
// SitemapConfig — moved from infrastructure::crawler::sitemap_config
// ============================================================================

/// Sitemap parser configuration.
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
    /// Supported compression types (default: [`crate::domain::CompressionType::Gzip`])
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
    fn test_builder_and_helpers_via_domain() {
        // Triangulate: helpers re-exported via domain are same as url_validation
        let cfg = SitemapConfig::builder().batch_size(5_000).build();
        assert_eq!(cfg.batch_size, 5_000);
        assert!(is_internal_link("https://example.com/a", "example.com"));
        let norm = normalize_url(
            "https://example.com/page#section",
            &NormalizeConfig {
                strip_www: true,
                query_policy: RemoveQueryParameters::All,
            },
        );
        assert_eq!(norm, "https://example.com/page");
    }
}
