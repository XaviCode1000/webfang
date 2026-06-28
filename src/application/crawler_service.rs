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
use tracing::{debug, error, info, instrument, span, warn, Level};
use url::Url;

use super::deduplicator::UrlDeduplicator;

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

// ============================================================================
// Legacy Crawl Functions (kept for backward compatibility)
// ============================================================================

/// Crawl a website starting from the seed URL
///
/// Following **async-no-lock-across-await**: Uses JoinSet for concurrency control
/// without redundant Semaphore (JoinSet already limits via tasks.len()).
/// Following **async-clone-before-await**: Clones config before async operations.
///
/// # Arguments
///
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(CrawlResult)` - Crawl result with discovered URLs
/// * `Err(CrawlError)` - Error during crawling
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::{domain::CrawlerConfig, application::crawl_site};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::builder(seed)
///     .max_depth(2)
///     .max_pages(50)
///     .build();
///
/// let result = crawl_site(config).await?;
/// println!("Crawled {} pages", result.total_pages);
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "crawl_site",
    skip(config),
    fields(
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages,
        delay_ms = config.delay_ms,
        concurrency = config.concurrency
    )
)]
pub async fn crawl_site(config: CrawlerConfig) -> Result<CrawlResult, CrawlError> {
    let span = span!(
        Level::INFO,
        "crawl_site",
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages
    );
    let _guard = span.enter();

    info!(
        "Starting crawl from {} with max_depth={} max_pages={}",
        config.seed_url, config.max_depth, config.max_pages
    );

    // Clone config for async safety (following async-clone-before-await)
    let config = Arc::new(config);
    let config_clone = Arc::clone(&config);

    // Create rate limiter using SharedRateLimiter (single source of truth)
    let rate_limiter_config =
        RateLimiterConfig::new(config_clone.delay_ms, config_clone.concurrency as u32);
    let rate_limiter = match SharedRateLimiter::new(&rate_limiter_config) {
        Ok(limiter) => limiter,
        Err(e) => return Err(CrawlError::Internal(e.to_string())),
    };

    // Create URL queue
    let queue = Arc::new(UrlQueue::new());

    // Add seed URL to queue
    let seed_discovered = DiscoveredUrl::html(
        config_clone.seed_url.clone(),
        0,
        config_clone.seed_url.clone(),
    );
    queue.push(seed_discovered).await;

    // Track visited URLs — lock-free DashSet<u64, ahash::RandomState> (8 B/key).
    // try_insert is synchronous/atomic; no Mutex/.await in the hot loop (D5).
    let visited = Arc::new(UrlDeduplicator::new());

    // Results collector - usa mpsc channel para lock-free collection
    // Capacidad basada en max_pages para evitar reallocs
    let results_collector =
        ResultsCollector::new(config_clone.max_pages, Some(config_clone.max_pages));
    // Sender clonado para los workers - puede ser compartido via Arc si es necesario
    let error_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut tasks = tokio::task::JoinSet::new();
    let mut url_queue = std::collections::VecDeque::new();
    url_queue.push_back(DiscoveredUrl::html(
        config_clone.seed_url.clone(),
        0,
        config_clone.seed_url.clone(),
    ));

    // Main crawl loop
    while !url_queue.is_empty() || !tasks.is_empty() {
        // Check if we've reached max pages (sin lock - atomic)
        if results_collector.is_full(config_clone.max_pages) {
            info!("Reached max pages limit: {}", config_clone.max_pages);
            break;
        }

        // Process completed tasks FIRST (non-blocking)
        while let Some(result) = tasks.try_join_next() {
            handle_crawl_result(result, &error_count);
        }

        // Drain discovered links from the deduplicated UrlQueue
        // into the main crawl loop's VecDeque work queue.
        // Fix Bug 5: discovered links were pushed to queue (Arc<UrlQueue>)
        // but never transferred to url_queue (VecDeque), so sub-paths
        // were never crawled.
        url_queue.append(&mut queue.drain_all().await);

        // Spawn new tasks up to concurrency limit
        while let Some(discovered_url) = url_queue.pop_front() {
            // Check concurrency limit
            if tasks.len() >= config_clone.concurrency {
                // Queue llena, re-encolar y break
                url_queue.push_front(discovered_url);
                break;
            }

            // Check if already visited — atomic, lock-free (no .await on the
            // dedup call; try_insert is synchronous per design D5).
            if !visited.try_insert(discovered_url.url.as_str()) {
                continue;
            }

            // Clone data for task (async-clone-before-await)
            let config_task = Arc::clone(&config);
            let queue_task = Arc::clone(&queue);
            let results_sender = results_collector.clone(); // Clone sender para este worker
            let visited_task = Arc::clone(&visited);
            let error_count_task = Arc::clone(&error_count);
            let rate_limiter_task = rate_limiter.clone(); // SharedRateLimiter is Clone (Arc internally)
            let discovered_url_task = discovered_url.clone();

            // Clone parent URL before moving discovered_url_task
            let parent_url = discovered_url_task.url.clone();

            // Spawn task
            tasks.spawn(async move {
                // Rate limiting
                rate_limiter_task.until_ready().await;

                let url_str = discovered_url_task.url.as_str().to_string();
                let url_depth = discovered_url_task.depth;

                debug!("Crawling: {} (depth={})", url_str, url_depth);

                // Fetch URL
                match fetch_url(&url_str, &config_task).await {
                    Ok(response) => {
                        // Add to results via channel (sin lock)
                        if let Err(e) = results_sender
                            .send(CrawlMessage::success(discovered_url_task))
                            .await
                        {
                            debug!("Failed to send result: {}", e);
                        }

                        // Extract links and add to queue
                        if url_depth < config_task.max_depth {
                            match extract_links(&response, &url_str) {
                                Ok(links) => {
                                    for link in links {
                                        let normalized = normalize_url(&link);
                                        if let Ok(parsed_url) = Url::parse(&normalized) {
                                            // Check if internal link
                                            if let Some(seed_domain) =
                                                config_task.seed_url.host_str()
                                            {
                                                if is_internal_link(&normalized, seed_domain) {
                                                    // Check if allowed by filters
                                                    if is_allowed(&normalized, &config_task) {
                                                        // Check if not visited — atomic,
                                                        // lock-free (no .await on try_insert).
                                                        if visited_task.try_insert(&normalized) {
                                                            let new_discovered =
                                                                DiscoveredUrl::html(
                                                                    parsed_url,
                                                                    url_depth + 1,
                                                                    parent_url.clone(),
                                                                );
                                                            queue_task.push(new_discovered).await;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                },
                                Err(e) => {
                                    warn!("Failed to extract links from {}: {}", url_str, e);
                                    error_count_task
                                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                },
                            }
                        }
                    },
                    Err(e) => {
                        error!("Failed to fetch {}: {}", url_str, e);
                        error_count_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        return Err(e);
                    },
                }

                Ok(())
            });
        }

        // If no tasks can be spawned and queue is not empty, wait for one task
        if tasks.len() >= config_clone.concurrency && !url_queue.is_empty() {
            if let Some(result) = tasks.join_next().await {
                handle_crawl_result(result, &error_count);
            }
        }
    }

    // Wait for remaining tasks
    while let Some(result) = tasks.join_next().await {
        handle_crawl_result(result, &error_count);
    }

    // Collect results via mpsc channel (shutdown limpio)
    let collected_urls = results_collector.collect().await;
    let total_pages = collected_urls.len();
    let errors = error_count.load(std::sync::atomic::Ordering::SeqCst);

    info!("Crawl complete: {} pages, {} errors", total_pages, errors);

    Ok(CrawlResult::new(collected_urls, total_pages, errors))
}

/// Handle result from a completed crawl task
///
/// Helper function to process task results.
fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<std::sync::atomic::AtomicUsize>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => {
            warn!("Task error: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
        Err(e) => {
            warn!("Task panicked: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        },
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
