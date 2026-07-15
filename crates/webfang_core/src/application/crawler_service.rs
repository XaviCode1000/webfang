//! ⚠️ **DEPRECATED since v0.5.0** ⚠️
//! This module is a re-export shim for backwards compatibility.
//! The actual implementation now lives in:
//! - `crate::application::crawler::engine` — crawl orchestration
//! - `crate::application::crawler::discovery` — URL discovery
//! - `crate::application::crawler::collector` — ResultsCollector (mpsc)
//!
//! Migrate imports to `use webfang::application::crawler::*;`

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, warn};
use url::Url;

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

/// Fetch and parse a sitemap.xml file (legacy — kept for backwards compatibility)
///
/// Prefer [`crawl_with_sitemap`] for new code.
#[deprecated(since = "0.4.0", note = "Use crawl_with_sitemap instead")]
pub async fn fetch_sitemap(base_url: &str) -> Result<Vec<String>, CrawlError> {
    info!("Fetching sitemap from {} (legacy method)", base_url);

    let sitemap_urls = [
        format!("{}/sitemap.xml", base_url.trim_end_matches('/')),
        format!("{}/sitemap_index.xml", base_url.trim_end_matches('/')),
        format!("{}/sitemap.xml.gz", base_url.trim_end_matches('/')),
    ];

    let mut all_urls = Vec::new();

    for sitemap_url in &sitemap_urls {
        debug!("Trying sitemap: {}", sitemap_url);

        let seed = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
        let config = Arc::new(CrawlerConfig::new(seed.clone()));
        let config_clone = Arc::clone(&config);

        match fetch_url(sitemap_url, &config_clone).await {
            Ok(response) => match super::crawler::parse_sitemap(&response, &seed) {
                Ok(urls) => {
                    info!("Found {} URLs in {}", urls.len(), sitemap_url);
                    all_urls.extend(urls);
                },
                Err(e) => {
                    warn!("Failed to parse sitemap {}: {}", sitemap_url, e);
                },
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
