//! Crawler infrastructure module
//!
//! Technical implementations for web crawling:
//! - HTTP client with rate limiting
//! - Link extraction from HTML
//! - Concurrent URL queue
//! - Sitemap parsing (FASE 3)
//! - Resource downloading with byte-weighted semaphore backpressure

pub mod batch_processor;
pub mod binary_utils;
pub mod compression_handler;
pub mod http_client;
pub mod link_extractor;
pub mod memory_manager;
pub mod resource_downloader;
pub mod retry_policy;
pub mod robots_utils;
pub mod sitemap_config;
pub mod sitemap_parser;
pub mod url_queue;
pub mod url_validator;

pub use binary_utils::{derive_filename_from_response, parse_content_disposition, percent_decode};
pub use http_client::{create_rate_limited_client, fetch_url};
pub use link_extractor::{extract_links, is_internal_link, normalize_url};
pub use resource_downloader::{
    DownloadConfig, DownloadedResource, PermitGuard, ResourceDownloader,
};
pub use robots_utils::{
    get_crawl_delay, is_allowed_by_robots, new_robots_cache, RobotsCache, RobotsRules,
};
pub use sitemap_config::{SitemapConfig, SitemapConfigBuilder};
pub use sitemap_parser::{resolve_url, SitemapError, SitemapParser};
pub use url_queue::{UrlQueue, UrlSource};
