//! Crawler service module (DEPRECATED)
//!
//! ⚠️ **DEPRECATED since v0.5.0** ⚠️
//! This module is kept for backwards compatibility ONLY.
//!
//! # Migration
//!
//! Replace:
//! ```rust
//! use rust_scraper::application::crawler_service::*;
//! ```
//!
//! With:
//! ```rust
//! use rust_scraper::application::crawler::{self, *};
//! ```
//!
//! Or access individual modules:
//! ```rust
//! use rust_scraper::application::crawler::discovery;
//! use rust_scraper::application::crawler::engine;
//! ```

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, warn};
use url::Url;

pub use crate::domain::{
    CorrelationId, CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl, ScrapedContent, ValidUrl,
};

pub use super::results_channel::{CrawlMessage, ResultsCollector};
pub use super::url_filter::is_allowed;
pub use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, normalize_url, UrlQueue,
};

pub use crate::infrastructure::crawler::{SitemapConfig, SitemapParser};

pub use crate::error::{Result as ScraperResult, ScraperError};
pub use crate::infrastructure::scraper::{fallback, readability};
pub use crate::ScraperConfig;

pub use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};

/// Resumable crawl state for large sitemaps
///
/// Maintains state between batches to enable resumption
/// after interruption (network failure, timeout, etc.)
///
/// Following **api-builder-pattern**: clear, self-documenting API
#[derive(Debug, Clone)]
pub struct CrawlState {
    /// Base URL being crawled
    pub base_url: String,
    /// Sitemap URL (if discovered)
    pub sitemap_url: Option<String>,
    /// URLs already processed (for deduplication)
    pub processed_urls: Vec<String>,
    /// Batch size for pagination
    pub batch_size: usize,
    /// Current batch offset
    pub offset: usize,
    /// Whether pagination is enabled
    pub pagination_enabled: bool,
    /// Last error (if any) for debugging
    pub last_error: Option<String>,
    /// W3C TraceContext correlation ID for distributed tracing
    pub correlation_id: Option<CorrelationId>,
}

impl CrawlState {
    /// Create new crawl state
    pub fn new(base_url: String, batch_size: usize) -> Self {
        Self {
            base_url,
            sitemap_url: None,
            processed_urls: Vec::new(),
            batch_size,
            offset: 0,
            pagination_enabled: batch_size > 0,
            last_error: None,
            correlation_id: Some(CorrelationId::new()),
        }
    }

    /// Mark URL as processed
    pub fn mark_processed(&mut self, url: String) {
        self.processed_urls.push(url);
        self.offset += 1;
    }

    /// Check if URL was already processed
    pub fn is_processed(&self, url: &str) -> bool {
        self.processed_urls.contains(&url.to_string())
    }

    /// Reset state for new crawl
    pub fn reset(&mut self) {
        self.processed_urls.clear();
        self.offset = 0;
        self.last_error = None;
        // Generate new correlation ID for new crawl job
        self.correlation_id = Some(CorrelationId::new());
    }

    /// Get correlation ID reference (clone-friendly)
    pub fn correlation_id(&self) -> Option<&CorrelationId> {
        self.correlation_id.as_ref()
    }

    /// Generate new correlation ID for this crawl job
    pub fn generate_correlation_id(&mut self) {
        self.correlation_id = Some(CorrelationId::new());
    }
}

/// Discover URLs from a single page
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
///
/// # Arguments
///
/// * `url` - URL to discover links from
/// * `depth` - Current depth in crawl tree
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(Vec<DiscoveredUrl>)` - Discovered URLs
/// * `Err(CrawlError)` - Error during discovery
#[deprecated(since = "0.4.0", note = "Use discover_urls_for_tui instead")]
pub async fn discover_urls(
    url: &str,
    depth: usize,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    debug!("Discovering URLs from {} at depth {}", url, depth);

    // Clone config for async safety
    let config = Arc::new(config.clone());
    let config_clone = Arc::clone(&config);

    // Fetch URL
    let response = fetch_url(url, &config_clone).await?;

    // Extract links
    let links = extract_links(&response, url)?;

    // Parse and filter URLs
    let base_url = Url::parse(url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
    let mut discovered = Vec::with_capacity(links.len());

    for link in links {
        let normalized = normalize_url(&link);
        if let Ok(parsed_url) = Url::parse(&normalized) {
            // Check if internal link
            if let Some(seed_domain) = config.seed_url.host_str() {
                if is_internal_link(&normalized, seed_domain) {
                    // Check if allowed
                    if is_allowed(&normalized, &config) {
                        discovered.push(DiscoveredUrl::html(
                            parsed_url,
                            depth as u8,
                            base_url.clone(),
                        ));
                    }
                }
            }
        }
    }

    Ok(discovered)
}

/// Fetch and parse a sitemap.xml file (legacy - kept for backwards compatibility)
///
/// Following **own-borrow-over-clone**: Accepts `&str`.
/// Following **xml-no-regex**: Uses quick-xml for streaming XML parsing.
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
///
/// # Returns
///
/// * `Ok(Vec<String>)` - List of URLs from sitemap
/// * `Err(CrawlError)` - Error during fetch or parse
#[deprecated(since = "0.4.0", note = "Use crawl_with_sitemap instead")]
pub async fn fetch_sitemap(base_url: &str) -> Result<Vec<String>, CrawlError> {
    info!("Fetching sitemap from {} (legacy method)", base_url);

    // Try common sitemap locations
    let sitemap_urls = [
        format!("{}/sitemap.xml", base_url.trim_end_matches('/')),
        format!("{}/sitemap_index.xml", base_url.trim_end_matches('/')),
        format!("{}/sitemap.xml.gz", base_url.trim_end_matches('/')),
    ];

    let mut all_urls = Vec::new();

    for sitemap_url in &sitemap_urls {
        debug!("Trying sitemap: {}", sitemap_url);

        // Create minimal config for sitemap fetch
        let seed = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
        let config = Arc::new(CrawlerConfig::new(seed.clone()));
        let config_clone = Arc::clone(&config);

        match fetch_url(sitemap_url, &config_clone).await {
            Ok(response) => {
                // Parse sitemap XML using quick-xml (streaming parser)
                // Pass seed as base_url for relative URL resolution
                match super::crawler::parse_sitemap(&response, &seed) {
                    Ok(urls) => {
                        info!("Found {} URLs in {}", urls.len(), sitemap_url);
                        all_urls.extend(urls);
                    },
                    Err(e) => {
                        warn!("Failed to parse sitemap {}: {}", sitemap_url, e);
                    },
                }
            },
            Err(e) => {
                debug!("Sitemap not found at {}: {}", sitemap_url, e);
            },
        }
    }

    if all_urls.is_empty() {
        warn!("No sitemap found for {}", base_url);
    }

    Ok(all_urls)
}
