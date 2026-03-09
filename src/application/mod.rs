//! Application layer — Use cases and orchestration
//!
//! This layer contains the business logic that orchestrates the domain objects
//! using infrastructure services. It depends on both domain and infrastructure.

pub mod crawler_service;
pub mod http_client;
pub mod scraper_service;
pub mod url_filter;

pub use crawler_service::{
    crawl_site, crawl_with_sitemap, discover_urls_for_tui, scrape_urls_for_tui,
};
pub use http_client::create_http_client;
pub use scraper_service::{
    scrape_multiple_with_limit, scrape_with_config, scrape_with_readability,
};
pub use url_filter::{extract_domain, is_allowed, is_excluded, is_internal_link, matches_pattern};
