//! Crawler module — crawling orchestration and result collection
//!
//! This module contains the crawler service and its supporting components.

pub mod checkpoint;
pub mod collector;
pub mod concurrency_level;
pub mod discovery;
pub mod engine;

pub use collector::{CrawlMessage, ResultsAdapter, ResultsCollector};
pub use concurrency_level::{ConcurrencyLevel, SharedConcurrencyLevel};
pub use discovery::{
    crawl_with_sitemap, discover_urls_for_tui, parse_sitemap, scrape_single_url_for_tui,
    scrape_urls_for_tui,
};
pub use engine::{crawl_site, crawl_site_with_options, EngineOptions};
