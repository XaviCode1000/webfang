//! Application layer — Use cases and orchestration
//!
//! This layer contains the business logic that orchestrates the domain objects
//! using infrastructure services. It depends on both domain and infrastructure.

pub mod container;
pub mod crawl_options;
pub mod crawl_result_repository;
pub mod crawler;
pub mod crawler_service;
pub mod deduplicator;
pub mod elastic_ingestion;
pub mod export_factory;
pub mod export_utils;
pub mod http_client;
pub mod rate_limiter;
pub mod results_channel;
pub mod scraper_service;
pub mod title_resolver;
pub mod url_filter;

pub use crawler::{
    crawl_site, crawl_with_sitemap, discover_urls_for_tui, scrape_single_url_for_tui,
    scrape_urls_for_tui,
};
pub use deduplicator::{normalize_url, UrlDeduplicator};
pub use http_client::create_http_client;
pub use rate_limiter::{RateLimiterConfig, SharedRateLimiter};
pub use results_channel::{CrawlMessage, ResultsAdapter, ResultsCollector};
pub use scraper_service::{
    detect_spa_content, scrape_multiple_with_limit, scrape_with_config, scrape_with_readability,
    SpaDetectionResult,
};
pub use title_resolver::resolve_title;
pub use url_filter::{extract_domain, is_allowed, is_excluded, is_internal_link, matches_pattern};
