//! ⚠️ **DEPRECATED since v0.5.0** ⚠️
//! This module is a re-export shim for backwards compatibility.
//! The actual implementation now lives in:
//! - `crate::application::crawler::engine` — crawl orchestration
//! - `crate::application::crawler::discovery` — URL discovery
//! - `crate::application::crawler::collector` — ResultsCollector (mpsc)
//!
//! Migrate imports to `use webfang_core::application::crawler::*;`

// --- Domain / infrastructure re-exports (unchanged) ---
pub use super::url_filter::is_allowed;
pub use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
pub use crate::domain::config::ScraperConfig;
pub use crate::domain::crawler_port::SitemapConfig;
pub use crate::domain::url_validation::{is_internal_link, normalize_url};
pub use crate::domain::{
    CorrelationId, CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl, ScrapedContent, ValidUrl,
};
pub use crate::error::{Result as ScraperResult, ScraperError};
pub use crate::infrastructure::crawler::{extract_links, fetch_url, SitemapParser, UrlQueue};
pub use crate::infrastructure::scraper::{fallback, readability};

// --- Crawler sub-module re-exports (canonical paths) ---
pub use super::crawler::collector::{ResultsAdapter, ResultsCollector};
pub use super::crawler::discovery::{
    crawl_with_sitemap, discover_urls_for_tui, scrape_single_url_for_tui,
};
pub use super::crawler::engine::crawl_site;
