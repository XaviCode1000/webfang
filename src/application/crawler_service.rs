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

use super::url_filter::is_allowed;
use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, normalize_url, UrlQueue,
};

// FASE 3: Sitemap support
use crate::infrastructure::crawler::sitemap_parser::{SitemapConfig, SitemapParser};

// FASE 4: TUI support - re-exports
use crate::error::{Result as ScraperResult, ScraperError};
use crate::infrastructure::scraper::{fallback, readability};
use crate::ScraperConfig;

/// Type alias for the rate limiter used in crawling
type CrawlRateLimiter = RateLimiter<NotKeyed, InMemoryState, QuantaClock>;

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

        let html = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Network error: {}", e))?;

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
        .map(|url| async { scrape_single_url(&client, url, config).await })
        .buffered(config.scraper_concurrency)
        .collect::<Vec<_>>()
        .await;

    // Collect results, propagating first error if any
    results.into_iter().collect()
}

/// Scrape a single URL
///
/// Helper function for scrape_urls_for_tui.
async fn scrape_single_url(
    client: &reqwest_middleware::ClientWithMiddleware,
    url: &Url,
    config: &ScraperConfig,
) -> ScraperResult<ScrapedContent> {
    debug!("Scraping: {}", url);

    // Fetch HTML
    let response = client
        .get(url.as_str())
        .send()
        .await
        .map_err(|e| ScraperError::Middleware(e.to_string()))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ScraperError::http(status, url.as_str()));
    }

    let html = response
        .text()
        .await
        .map_err(|e| ScraperError::Middleware(e.to_string()))?;

    // Try Readability first, fallback to plain text extraction
    match readability::parse(&html, Some(url.as_str())) {
        Ok(article) => {
            let assets = download_assets_if_enabled(&html, url, config).await?;

            Ok(ScrapedContent {
                title: article.title,
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                html: Some(html),
                assets,
            })
        }
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
        }
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

    // Results collector
    let results = Arc::new(Mutex::new(Vec::new()));
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
        // Check if we've reached max pages
        {
            let results_guard = results.lock().await;
            if results_guard.len() >= config_clone.max_pages {
                info!("Reached max pages limit: {}", config_clone.max_pages);
                drop(results_guard);
                break;
            }
        }

        // Process completed tasks FIRST (non-blocking)
        while let Some(result) = tasks.try_join_next() {
            handle_crawl_result(result, &error_count);
        }

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
            let results_task = Arc::clone(&results);
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
                        // Add to results
                        {
                            let mut results_guard = results_task.lock().await;
                            results_guard.push(discovered_url_task);
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
                                }
                                Err(e) => {
                                    warn!("Failed to extract links from {}: {}", url_str, e);
                                    error_count_task
                                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to fetch {}: {}", url_str, e);
                        error_count_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        return Err(e);
                    }
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

    // Collect results
    let results_guard = results.lock().await;
    let total_pages = results_guard.len();
    let errors = error_count.load(std::sync::atomic::Ordering::SeqCst);

    info!("Crawl complete: {} pages, {} errors", total_pages, errors);

    Ok(CrawlResult::new(results_guard.clone(), total_pages, errors))
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
        }
        Ok(Err(e)) => {
            warn!("Task error: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Err(e) => {
            warn!("Task panicked: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
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
    _config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    info!("Crawling with sitemap for {}", base_url);

    // Auto-discover sitemap URL if not provided
    let sitemap_url = match sitemap_url {
        Some(url) => url.to_string(),
        None => discover_sitemap_url(base_url).await?,
    };

    tracing::info!("Using sitemap: {}", sitemap_url);

    // Create sitemap parser with config
    // Following api-builder-pattern: builder API
    let parser = SitemapParser::with_config(
        SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(3)
            .concurrency(5)
            .build(),
    );

    // Parse sitemap
    let urls = parser.parse_from_url(&sitemap_url).await.map_err(|e| {
        tracing::error!("Failed to parse sitemap {}: {}", sitemap_url, e);
        CrawlError::Sitemap(e.to_string())
    })?;

    // Convert to DiscoveredUrl
    // Following own-borrow-over-clone: use Url directly, not String
    let base_url =
        Url::parse(&sitemap_url).unwrap_or_else(|_| Url::parse("https://example.com").unwrap());
    let discovered = urls
        .into_iter()
        .map(|url| DiscoveredUrl::html(url, 0, base_url.clone()))
        .collect();

    Ok(discovered)
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

    if let Ok(response) = reqwest::get(robots_url).await {
        if response.status().is_success() {
            if let Ok(content) = response.text().await {
                // Extract Sitemap: directive
                for line in content.lines() {
                    if line.to_lowercase().starts_with("sitemap:") {
                        if let Some(sitemap) = line
                            .strip_prefix("Sitemap:")
                            .or_else(|| line.strip_prefix("sitemap:"))
                        {
                            let sitemap = sitemap.trim();
                            tracing::debug!("Found sitemap in robots.txt: {}", sitemap);
                            return Ok(sitemap.to_string());
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
        if let Ok(response) = reqwest::Client::new().head(sitemap_str).send().await {
            if response.status().is_success() {
                tracing::debug!("Found sitemap at fallback location: {}", sitemap_str);
                return Ok(sitemap_str.to_string());
            }
        }
    }

    // Last resort: return default location (may 404, but caller handles)
    Ok(base
        .join("/sitemap.xml")
        .map_err(|e| CrawlError::InvalidUrl(e.to_string()))?
        .to_string())
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
        let config = Arc::new(CrawlerConfig::new(seed));
        let config_clone = Arc::clone(&config);

        match fetch_url(sitemap_url, &config_clone).await {
            Ok(response) => {
                // Parse sitemap XML using quick-xml (streaming parser)
                match parse_sitemap(&response) {
                    Ok(urls) => {
                        info!("Found {} URLs in {}", urls.len(), sitemap_url);
                        all_urls.extend(urls);
                    }
                    Err(e) => {
                        warn!("Failed to parse sitemap {}: {}", sitemap_url, e);
                    }
                }
            }
            Err(e) => {
                debug!("Sitemap not found at {}: {}", sitemap_url, e);
            }
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
fn parse_sitemap(xml_content: &str) -> Result<Vec<String>, CrawlError> {
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
            }
            Ok(Event::End(ref e)) => {
                if e.name().as_ref() == b"loc" {
                    in_loc = false;
                }
            }
            Ok(Event::Text(ref e)) if in_loc => {
                let text = e.unescape().map_err(|e| CrawlError::Parse(e.to_string()))?;
                let url = text.trim().to_string();
                if !url.is_empty() {
                    urls.push(url);
                }
            }
            Ok(Event::CData(ref e)) if in_loc => {
                // Handle CDATA sections - BytesCData derefs to [u8]
                let url = String::from_utf8_lossy(e).trim().to_string();
                if !url.is_empty() {
                    urls.push(url);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(CrawlError::Parse(e.to_string())),
            _ => {}
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

        let urls = parse_sitemap(xml).unwrap();
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

        let urls = parse_sitemap(xml).unwrap();
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

        let urls = parse_sitemap(xml).unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com/page1");
    }

    #[test]
    fn test_parse_sitemap_xml_empty() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

        let urls = parse_sitemap(xml).unwrap();
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
