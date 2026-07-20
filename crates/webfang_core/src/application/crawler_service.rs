//! ⚠️ **DEPRECATED since v0.5.0** ⚠️
//! This module is a re-export shim for backwards compatibility.
//! The actual implementation now lives in:
//! - `crate::application::crawler::engine` — crawl orchestration
//! - `crate::application::crawler::discovery` — URL discovery
//! - `crate::application::crawler::collector` — ResultsCollector (mpsc)
//!
//! Migrate imports to `use webfang::application::crawler::*;`

// --- Domain / infrastructure re-exports (unchanged) ---
pub use super::url_filter::is_allowed;
pub use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
pub use crate::domain::{
    CorrelationId, CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl, ScrapedContent, ValidUrl,
};
pub use crate::error::{Result as ScraperResult, ScraperError};
pub use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, normalize_url, UrlQueue,
};
pub use crate::infrastructure::crawler::{SitemapConfig, SitemapParser};
pub use crate::infrastructure::scraper::{fallback, readability};
pub use crate::ScraperConfig;

// --- Crawler sub-module re-exports (canonical paths) ---
pub use super::crawler::collector::{CrawlMessage, ResultsAdapter, ResultsCollector};
pub use super::crawler::discovery::{
    crawl_with_sitemap, discover_urls_for_tui, scrape_single_url_for_tui, scrape_urls_for_tui,
};
pub use super::crawler::engine::crawl_site;
