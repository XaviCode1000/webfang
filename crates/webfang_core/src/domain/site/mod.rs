//! Site configuration
//!
//! Configuration and builder for crawling a specific site.

mod crawler_config;

/// Re-exported for the intra-crate sitemap boundary (#1190): the live
/// discovery path resolves [`SitemapConfig`] from the config instead of
/// reading the raw `use_sitemap` + `sitemap_url` pair.
pub(crate) use crawler_config::SitemapConfig;
pub use crawler_config::{CrawlerConfig, CrawlerConfigBuilder};
