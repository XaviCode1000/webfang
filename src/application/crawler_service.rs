//! Crawler service module
//!
//! Main crawling orchestration logic.
//!
//! # Rules Applied
//!
//! - **async-no-lock-across-await**: Uses JoinSet for concurrency control
//!   without redundant Semaphore.
//! - **async-clone-before-await**: Clones config before async operations.
//! - **err-anyhow-for-applications**: Result with anyhow for application layer
//! - **own-borrow-over-clone**: Accept `&str` not `&String`
//! - **mem-with-capacity**: Vec::with_capacity when size is known
//! - **xml-no-regex**: Uses quick-xml for streaming XML parsing
//! - **async-mpsc-results**: Uses mpsc channel for lock-free result collection

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use governor::{
    clock::QuantaClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};
use url::Url;

use crate::domain::{
    CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl, ScrapedContent, ValidUrl,
};

use super::results_channel::{CrawlMessage, ResultsCollector};
use super::url_filter::is_allowed;
use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, normalize_url, UrlQueue,
};

// FASE 3: Sitemap support
use crate::infrastructure::crawler::{SitemapConfig, SitemapParser};

// FASE 4: TUI support - re-exports
use crate::error::{Result as ScraperResult, ScraperError};
use crate::infrastructure::scraper::{fallback, readability};
use crate::ScraperConfig;

/// Type alias for the rate limiter used in crawling
type CrawlRateLimiter = RateLimiter<NotKeyed, InMemoryState, QuantaClock>;

// ============================================================================
// FASE 4: Progress Tracking and Resumable Processing Types
// ============================================================================

/// Progress information for sitemap crawling operations
///
/// Tracks batch progress, completion percentage, and URL counts
/// for long-running sitemap parsing operations.
///
/// Following **api-builder-pattern**: clear, self-documenting API
#[derive(Debug, Clone)]
pub struct CrawlProgress {
    /// Current batch number (0-indexed)
    pub current_batch: usize,
    /// Total number of batches
    pub total_batches: usize,
    /// Completion percentage (0.0 to 100.0)
    pub percentage: f32,
    /// Number of URLs processed in current batch
    pub urls_in_batch: usize,
    /// Total URLs discovered so far
    pub total_urls: usize,
    /// Whether the crawl is complete
    pub is_complete: bool,
}

impl Default for CrawlProgress {
    fn default() -> Self {
        Self {
            current_batch: 0,
            total_batches: 0,
            percentage: 0.0,
            urls_in_batch: 0,
            total_urls: 0,
            is_complete: false,
        }
    }
}

impl CrawlProgress {
    /// Create new progress tracker with total URL estimate
    pub fn new(total_urls_estimate: usize, batch_size: usize) -> Self {
        let total_batches = total_urls_estimate.div_ceil(batch_size);
        Self {
            current_batch: 0,
            total_batches,
            percentage: 0.0,
            urls_in_batch: 0,
            total_urls: 0,
            is_complete: false,
        }
    }

    /// Update progress after processing a batch
    pub fn update(&mut self, urls_in_batch: usize, total_so_far: usize) {
        self.urls_in_batch = urls_in_batch;
        self.total_urls = total_so_far;
        self.current_batch += 1;
        self.percentage = if self.total_batches > 0 {
            ((self.current_batch as f32) / (self.total_batches as f32)) * 100.0
        } else {
            100.0
        };
        self.is_complete = self.current_batch >= self.total_batches;
    }
}

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
    }
}

// ============================================================================
// FASE 4: TUI Support - Separated Discover/Scrape Use Cases
// ============================================================================

/// Discover URLs from a website without downloading content
///
/// This is the first step in the TUI workflow:
/// 1. Discover all URLs from sitemap or DOM scraping
/// 2. Return Vec<Url> for interactive selection
/// 3. User selects which URLs to scrape
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
///
/// # Arguments
///
/// * `base_url` - Base URL to discover from
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(Vec<Url>)` - Discovered URLs (owned)
/// * `Err(anyhow::Error)` - Error during discovery
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::{application::discover_urls_for_tui, domain::CrawlerConfig};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::new(seed);
///
/// let urls = discover_urls_for_tui("https://example.com", &config).await?;
/// println!("Found {} URLs", urls.len());
/// # Ok(())
/// # }
/// ```
pub async fn discover_urls_for_tui(
    base_url: &str,
    config: &CrawlerConfig,
) -> anyhow::Result<Vec<Url>> {
    info!("Discovering URLs from {}", base_url);

    // If sitemap enabled, use sitemap (preferred)
    if config.use_sitemap {
        let discovered =
            crawl_with_sitemap(base_url, config.sitemap_url.as_deref(), config).await?;
        Ok(discovered.into_iter().map(|d| d.url).collect())
    } else {
        // DOM scraping - extract links from single page
        let client = super::create_http_client()?;

        info!("Fetching {} for link extraction", base_url);
        let response = client
            .get(base_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP error: {}", e))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("unknown"))
            .unwrap_or("unknown");
        let content_length = response
            .headers()
            .get("content-length")
            .map(|v| v.to_str().unwrap_or("0"))
            .unwrap_or("0");

        debug!(
            "Response: status={}, content-type={}, content-length={}",
            status, content_type, content_length
        );

        let html = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Network error: {}", e))?;

        debug!("Received HTML: {} bytes", html.len());

        let base = Url::parse(base_url).map_err(|e| anyhow::anyhow!("Invalid URL: {}", e))?;

        // Extract links
        let links =
            extract_links(&html, base_url).map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;

        // Filter and normalize URLs
        let mut urls = Vec::new();
        for link in links {
            let normalized = normalize_url(&link);
            if let Ok(parsed_url) = Url::parse(&normalized) {
                // Check if internal link
                if let Some(seed_domain) = base.host_str() {
                    if is_internal_link(&normalized, seed_domain) {
                        // Check if allowed by filters
                        if is_allowed(&normalized, config) {
                            urls.push(parsed_url);
                        }
                    }
                }
            }
        }

        info!("Discovered {} URLs from {}", urls.len(), base_url);
        Ok(urls)
    }
}

/// Scrape/download specific URLs
///
/// This is the second step in the TUI workflow:
/// 1. User selects URLs via TUI
/// 2. This function downloads and extracts content
///
/// Following **own-borrow-over-clone**: Accepts `&[Url]` not `&Vec<Url>`.
/// Following **async-no-lock-across-await**: Uses stream with buffer_unordered.
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
///
/// # Arguments
///
/// * `urls` - Slice of URLs to scrape (borrowed)
/// * `config` - Scraper configuration
///
/// # Returns
///
/// * `Ok(Vec<ScrapedContent>)` - Scraped content from each URL
/// * `Err(ScraperError)` - Error during scraping
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::{application::scrape_urls_for_tui, ScraperConfig};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let urls = vec![
///     Url::parse("https://example.com/1")?,
///     Url::parse("https://example.com/2")?,
/// ];
/// let config = ScraperConfig::default();
/// let results = scrape_urls_for_tui(&urls, &config).await?;
/// # Ok(())
/// # }
/// ```
pub async fn scrape_urls_for_tui(
    urls: &[Url],
    config: &ScraperConfig,
) -> ScraperResult<Vec<ScrapedContent>> {
    use futures::stream::{self, StreamExt};

    info!("Scraping {} URLs", urls.len());

    let client = super::create_http_client()?;

    // Stream processing with concurrency control
    // Following async-no-lock-across-await: buffer_unordered handles concurrency
    let results = stream::iter(urls)
        .map(|url| async { scrape_single_url_for_tui(&client, url, config).await })
        .buffered(config.scraper_concurrency)
        .collect::<Vec<_>>()
        .await;

    // Collect results, propagating first error if any
    results.into_iter().collect()
}

/// Scrape a single URL
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
///
/// # Arguments
///
/// * `client` - HTTP client to use for requests
/// * `url` - URL to scrape
/// * `config` - Scraper configuration
///
/// # Returns
///
/// * `Ok(ScrapedContent)` - Scraped content from the URL
/// * `Err(ScraperError)` - Error during scraping
pub async fn scrape_single_url_for_tui(
    client: &wreq::Client,
    url: &Url,
    config: &ScraperConfig,
) -> ScraperResult<ScrapedContent> {
    debug!("Scraping: {}", url);

    // Fetch HTML
    let response = client
        .get(url.as_str())
        .send()
        .await
        .map_err(|e| ScraperError::Network(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ScraperError::http(status.as_u16(), url.as_str()));
    }

    let html = response
        .text()
        .await
        .map_err(|e| ScraperError::Network(e.to_string()))?;

    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = crate::infrastructure::converter::html_cleaner::clean_html(&html);

    // Try Readability first, fallback to plain text extraction
    match readability::parse(&cleaned_html, Some(url.as_str())) {
        Ok(article) => {
            let assets = download_assets_if_enabled(&html, url, config).await?;

            Ok(ScrapedContent {
                title: article.title,
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                html: Some(article.content),
                assets,
            })
        },
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            let fallback_content = fallback::extract_text(&html);
            let assets = download_assets_if_enabled(&html, url, config).await?;

            Ok(ScrapedContent {
                title: url
                    .host_str()
                    .ok_or_else(|| ScraperError::invalid_url(format!("URL missing host: {}", url)))?
                    .to_string(),
                content: fallback_content,
                url: ValidUrl::new(url.clone()),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html),
                assets,
            })
        },
    }
}

/// Download assets if enabled in config
///
/// Helper function to conditionally download assets.
#[cfg(any(feature = "images", feature = "documents"))]
async fn download_assets_if_enabled(
    html: &str,
    url: &Url,
    config: &ScraperConfig,
) -> ScraperResult<Vec<crate::domain::DownloadedAsset>> {
    if config.has_downloads() {
        tracing::debug!("Calling download_all for assets...");
        use crate::infrastructure::scraper::asset_download::download_all;

        download_all(html, url, config).await
    } else {
        tracing::debug!("has_downloads is false, skipping asset download");
        Ok(Vec::new())
    }
}

/// Download assets stub when features are disabled
#[cfg(not(any(feature = "images", feature = "documents")))]
async fn download_assets_if_enabled(
    _html: &str,
    _url: &Url,
    _config: &ScraperConfig,
) -> ScraperResult<Vec<crate::domain::DownloadedAsset>> {
    Ok(Vec::new())
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
pub async fn crawl_site(config: CrawlerConfig) -> Result<CrawlResult, CrawlError> {
    info!(
        "Starting crawl from {} with max_depth={} max_pages={}",
        config.seed_url, config.max_depth, config.max_pages
    );

    // Clone config for async safety (following async-clone-before-await)
    let config = Arc::new(config);
    let config_clone = Arc::clone(&config);

    // Create rate limiter (governor) - shared across tasks
    let quota = Quota::with_period(Duration::from_millis(config_clone.delay_ms))
        .unwrap()
        .allow_burst(NonZeroU32::new(config_clone.concurrency as u32).unwrap());
    let rate_limiter: Arc<CrawlRateLimiter> = Arc::new(RateLimiter::direct(quota));

    // Create URL queue
    let queue = Arc::new(UrlQueue::new());

    // Add seed URL to queue
    let seed_discovered = DiscoveredUrl::html(
        config_clone.seed_url.clone(),
        0,
        config_clone.seed_url.clone(),
    );
    queue.push(seed_discovered);

    // Track visited URLs
    let visited = Arc::new(Mutex::new(HashSet::<String>::new()));

    // Results collector - usa mpsc channel para lock-free collection
    // Capacidad basada en max_pages para evitar reallocs
    let results_collector = ResultsCollector::new(config_clone.max_pages, Some(config_clone.max_pages));
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
        url_queue.append(&mut queue.drain_all());

        // Spawn new tasks up to concurrency limit
        while let Some(discovered_url) = url_queue.pop_front() {
            // Check concurrency limit
            if tasks.len() >= config_clone.concurrency {
                // Queue llena, re-encolar y break
                url_queue.push_front(discovered_url);
                break;
            }

            // Check if already visited
            {
                let visited_guard = visited.lock().await;
                if visited_guard.contains(discovered_url.url.as_str()) {
                    drop(visited_guard);
                    continue;
                }
            }

            // Clone data for task (async-clone-before-await)
            let config_task = Arc::clone(&config);
            let queue_task = Arc::clone(&queue);
            let results_sender = results_collector.clone(); // Clone sender para este worker
            let visited_task = Arc::clone(&visited);
            let error_count_task = Arc::clone(&error_count);
            let rate_limiter_task = Arc::clone(&rate_limiter);
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
                        if let Err(e) = results_sender.send(CrawlMessage::success(discovered_url_task)).await {
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
                                                        // Check if not visited
                                                        let visited_guard =
                                                            visited_task.lock().await;
                                                        if !visited_guard.contains(&normalized) {
                                                            drop(visited_guard);

                                                            let new_discovered =
                                                                DiscoveredUrl::html(
                                                                    parsed_url,
                                                                    url_depth + 1,
                                                                    parent_url.clone(),
                                                                );
                                                            queue_task.push(new_discovered);
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

/// Crawl site using sitemap (preferred method - FASE 3)
///
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **api-builder-pattern**: Uses SitemapConfig builder.
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
/// * `sitemap_url` - Optional explicit sitemap URL (auto-discovers if None)
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(Vec<DiscoveredUrl>)` - URLs discovered from sitemap
/// * `Err(CrawlError)` - Error during sitemap fetch or parse
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::application::crawl_with_sitemap;
/// use rust_scraper::domain::CrawlerConfig;
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::new(seed);
///
/// let urls = crawl_with_sitemap("https://example.com", None, &config).await?;
/// println!("Found {} URLs from sitemap", urls.len());
/// # Ok(())
/// # }
/// ```
pub async fn crawl_with_sitemap(
    base_url: &str,
    sitemap_url: Option<&str>,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    crawl_with_sitemap_internal(base_url, sitemap_url, config).await
}

/// Crawl with sitemap (internal version with progress tracking)
///
/// This is the internal implementation that supports optional progress tracking.
/// The public `crawl_with_sitemap` function calls this one.
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **err-anyhow-for-applications**: Uses Result with anyhow.
#[allow(unused_variables)]
async fn crawl_with_sitemap_internal(
    base_url: &str,
    sitemap_url: Option<&str>,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    info!("Crawling with sitemap for {}", base_url);

    // Use default batch size (10,000) - SitemapConfig handles pagination
    // CrawlerConfig doesn't have batch_size, we use SitemapConfig for that
    const DEFAULT_BATCH_SIZE: usize = 10_000;

    // Auto-discover sitemap URL if not provided
    let sitemap_url = match sitemap_url {
        Some(url) if !url.is_empty() => {
            tracing::info!("Sitemap URL provided: {}", url);
            url.to_string()
        },
        _ => {
            tracing::info!("Auto-discovering sitemap URL for {}", base_url);
            match discover_sitemap_url(base_url).await {
                Ok(url) => {
                    tracing::info!("Discovered sitemap URL: {}", url);
                    url
                },
                Err(CrawlError::SitemapNotFound(_)) => {
                    tracing::warn!("no sitemap found, switching to standard crawling");
                    return Ok(Vec::new());
                },
                Err(e) => return Err(e),
            }
        },
    };

    tracing::info!("Using sitemap: {}", sitemap_url);

    // Create sitemap parser with config (including pagination settings)
    // Following api-builder-pattern: builder API
    let parser = SitemapParser::with_config(
        SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(3)
            .concurrency(5)
            .batch_size(DEFAULT_BATCH_SIZE)
            .pagination_enabled(true)
            .build(),
    );

    // Parse sitemap
    let urls = parser.parse_from_url(&sitemap_url).await.map_err(|e| {
        tracing::error!("Failed to parse sitemap {}: {}", sitemap_url, e);
        CrawlError::Sitemap(e.to_string())
    })?;

    let total_urls = urls.len();
    tracing::info!("Parsed {} total URLs from sitemap", total_urls);

    // Validate sitemap relevance: check if any URLs share a path prefix
    // with the target URL. This handles cases where robots.txt points to
    // an unrelated sitemap (e.g. blog sitemap for a docs site).
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
    let target_path = base.path().to_string();
    let relevant_urls: Vec<_> = urls
        .into_iter()
        .filter(|url| url.path().starts_with(&target_path))
        .collect();

    // If no relevant URLs found, try sub-path sitemaps as fallback
    if relevant_urls.is_empty() {
        tracing::warn!(
            "sitemap {} no tiene URLs que coincidan con la ruta objetivo {}, intentando sitemaps de subruta",
            sitemap_url,
            target_path
        );
        return crawl_with_subpath_sitemaps(base_url, &base, &parser).await;
    }

    // Following own-borrow-over-clone: use Url directly, not String
    // Use explicit type annotation for type inference
    let discovered: Vec<DiscoveredUrl> = relevant_urls
        .into_iter()
        .map(|url| DiscoveredUrl::html(url, 0, base.clone()))
        .collect();

    Ok(discovered)
}

/// Try sub-path sitemaps when the discovered sitemap has no relevant URLs
///
/// For nested sites like `https://example.com/docs/en/`, this tries
/// `/docs/sitemap.xml`, `/docs/en/sitemap.xml`, etc.
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
/// Following **err-no-unwrap-prod**: Proper error handling throughout.
async fn crawl_with_subpath_sitemaps(
    base_url: &str,
    base: &Url,
    parser: &SitemapParser,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    let path = base.path();
    let segments: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut all_urls = Vec::new();

    // Try up to 3 path levels: /docs, /docs/en, /docs/en/quickstart
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for sitemap_name in &["sitemap.xml", "sitemap_index.xml"] {
            let candidate = format!("/{}/{}", sub_path, sitemap_name);
            if let Ok(sitemap_url) = base.join(&candidate) {
                let sitemap_str = sitemap_url.as_str();
                tracing::debug!("Trying sub-path sitemap: {}", sitemap_str);
                if let Ok(response) = wreq::Client::new().head(sitemap_str).send().await {
                    if response.status().is_success() {
                        tracing::info!("Found sub-path sitemap: {}", sitemap_str);
                        if let Ok(urls) = parser.parse_from_url(sitemap_str).await {
                            tracing::info!(
                                "Parsed {} URLs from sub-path sitemap {}",
                                urls.len(),
                                sitemap_str
                            );
                            all_urls.extend(urls);
                        }
                    }
                }
            }
        }
    }

    if all_urls.is_empty() {
        tracing::warn!("no se encontraron sitemaps de subruta para {}", base_url);
        Ok(Vec::new())
    } else {
        Ok(all_urls
            .into_iter()
            .map(|url| DiscoveredUrl::html(url, 0, base.clone()))
            .collect())
    }
}

/// Auto-discover sitemap URL from robots.txt or fallback
///
/// Following **own-borrow-over-clone**: Accepts `&str`.
/// Following **security-no-unwrap-in-prod**: Proper error handling.
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
///
/// # Returns
///
/// * `Ok(String)` - Discovered sitemap URL
/// * `Err(CrawlError)` - Error during discovery
async fn discover_sitemap_url(base_url: &str) -> Result<String, CrawlError> {
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

    // Try robots.txt first
    let robots_url = base
        .join("/robots.txt")
        .map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

    tracing::info!("Checking robots.txt: {}", robots_url);
    if let Ok(response) = wreq::get(robots_url.as_str()).send().await {
        tracing::info!("robots.txt status: {}", response.status());
        if response.status().is_success() {
            if let Ok(content) = response.text().await {
                tracing::info!(
                    "robots.txt content (first 500 chars):\n{}",
                    &content[..content.len().min(500)]
                );
                // Extract Sitemap: directive
                for line in content.lines() {
                    if line.to_lowercase().starts_with("sitemap:") {
                        if let Some(sitemap) = line
                            .strip_prefix("Sitemap:")
                            .or_else(|| line.strip_prefix("sitemap:"))
                        {
                            let sitemap = sitemap.trim();
                            // Resolve relative URLs from robots.txt against base
                            let resolved = if sitemap.starts_with("http://")
                                || sitemap.starts_with("https://")
                            {
                                Url::parse(sitemap).ok()
                            } else {
                                base.join(sitemap).ok()
                            };
                            if let Some(url) = resolved {
                                tracing::debug!("Found sitemap in robots.txt: {}", url);
                                return Ok(url.to_string());
                            } else {
                                tracing::warn!("Invalid sitemap URL in robots.txt: {}", sitemap);
                            }
                        }
                    }
                }
            }
        }
    }

    tracing::debug!("No sitemap found in robots.txt, trying fallback locations");

    // Fallback: try common sitemap locations
    let fallback_urls = [
        "/sitemap.xml",
        "/sitemap_index.xml",
        "/sitemap.xml.gz",
        "/sitemap/sitemap.xml",
    ];

    for path in &fallback_urls {
        let sitemap_url = base
            .join(path)
            .map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
        let sitemap_str = sitemap_url.as_str();

        // Quick HEAD request to check if exists
        tracing::info!("Trying fallback sitemap: {}", sitemap_str);
        if let Ok(response) = wreq::Client::new().head(sitemap_str).send().await {
            tracing::info!("  Status: {}", response.status());
            if response.status().is_success() {
                tracing::debug!("Found sitemap at fallback location: {}", sitemap_str);
                return Ok(sitemap_str.to_string());
            }
        }
    }

    // GAP 5 (Bug #30): Try sub-path sitemaps for nested sites
    // e.g. https://example.com/docs/en/ → /docs/sitemap.xml, /docs/en/sitemap.xml
    let path = base.path();
    let segments: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for sitemap_name in &["sitemap.xml", "sitemap_index.xml"] {
            let candidate = format!("/{}/{}", sub_path, sitemap_name);
            if let Ok(sitemap_url) = base.join(&candidate) {
                let sitemap_str = sitemap_url.as_str();
                tracing::debug!("Trying sub-path sitemap: {}", sitemap_str);
                if let Ok(response) = wreq::Client::new().head(sitemap_str).send().await {
                    if response.status().is_success() {
                        tracing::info!("Found sitemap at sub-path: {}", sitemap_str);
                        return Ok(sitemap_str.to_string());
                    }
                }
            }
        }
    }
    // No sitemap found - return error instead of guessing
    tracing::warn!("no sitemap found for {}", base_url);
    Err(CrawlError::SitemapNotFound(base_url.to_string()))
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
                match parse_sitemap(&response, &seed) {
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

/// Parse sitemap XML content using quick-xml (streaming parser)
///
/// Following **xml-no-regex**: Uses quick-xml instead of regex for XML parsing.
/// Following **mem-stream-processing**: Streaming approach avoids loading entire DOM.
///
/// # Arguments
///
/// * `xml_content` - XML content of the sitemap
///
/// # Returns
///
/// * `Ok(Vec<String>)` - List of URLs
/// * `Err(CrawlError)` - Parse error
fn parse_sitemap(xml_content: &str, base_url: &Url) -> Result<Vec<String>, CrawlError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml_content);
    let mut buf = Vec::new();
    let mut urls = Vec::new();
    let mut in_loc = false;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                if e.name().as_ref() == b"loc" {
                    in_loc = true;
                }
            },
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"loc" {
                    in_loc = false;
                }
            },
            Ok(Event::Text(ref e)) if in_loc => {
                let text = e.unescape().map_err(|e| CrawlError::Parse(e.to_string()))?;
                let url_str = text.trim();
                if !url_str.is_empty() {
                    // Resolve relative URLs against base_url
                    // Following url-join-relative: use base_url.join() for relative paths
                    let resolved =
                        if url_str.starts_with("http://") || url_str.starts_with("https://") {
                            Url::parse(url_str).ok()
                        } else {
                            base_url.join(url_str).ok()
                        };
                    if let Some(url) = resolved {
                        urls.push(url.to_string());
                    }
                }
            },
            Ok(Event::CData(ref e)) if in_loc => {
                // Handle CDATA sections - BytesCData derefs to [u8]
                let url_str = String::from_utf8_lossy(e).trim().to_string();
                if !url_str.is_empty() {
                    // Resolve relative URLs against base_url
                    let resolved =
                        if url_str.starts_with("http://") || url_str.starts_with("https://") {
                            Url::parse(&url_str).ok()
                        } else {
                            base_url.join(&url_str).ok()
                        };
                    if let Some(url) = resolved {
                        urls.push(url.to_string());
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(CrawlError::Parse(e.to_string())),
            _ => {},
        }
    }

    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sitemap_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url>
        <loc>https://example.com/page1</loc>
    </url>
    <url>
        <loc>https://example.com/page2</loc>
    </url>
    <url>
        <loc>https://example.com/page3</loc>
    </url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://example.com/page1");
        assert_eq!(urls[1], "https://example.com/page2");
        assert_eq!(urls[2], "https://example.com/page3");
    }

    #[test]
    fn test_parse_sitemap_with_cdata() {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc><![CDATA[https://example.com/page1]]></loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://example.com/page1".to_string()));
        assert!(urls.contains(&"https://example.com/page2".to_string()));
    }

    #[test]
    fn test_parse_sitemap_with_namespaces() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:xhtml="http://www.w3.org/1999/xhtml">
    <url>
        <loc>https://example.com/page1</loc>
    </url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com/page1");
    }

    #[test]
    fn test_parse_sitemap_xml_empty() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert!(urls.is_empty());
    }

    #[tokio::test]
    #[allow(deprecated)]
    async fn test_discover_urls_invalid_url() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::new(seed);

        let result = discover_urls("not-a-valid-url", 0, &config).await;
        assert!(result.is_err());
    }
}
