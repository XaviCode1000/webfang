//! Crawler service module
//!
//! Main crawling orchestration logic.
//!
//! # Rules Applied
//!
//! - **async-no-lock-across-await**: Uses JoinSet for concurrency control
//!   without redundant Semaphore.
//! - **async-clone-before-await**: Clones config before async operations.
//! - **err-anyhow-for-apps**: Result with anyhow for application layer
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

use crate::domain::{CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl};

use super::url_filter::is_allowed;
use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, normalize_url, UrlQueue,
};

/// Type alias for the rate limiter used in crawling
type CrawlRateLimiter = RateLimiter<NotKeyed, InMemoryState, QuantaClock>;

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

/// Fetch and parse a sitemap.xml file
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
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::application::fetch_sitemap;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let urls = fetch_sitemap("https://example.com").await?;
/// println!("Found {} URLs in sitemap", urls.len());
/// # Ok(())
/// # }
/// ```
pub async fn fetch_sitemap(base_url: &str) -> Result<Vec<String>, CrawlError> {
    info!("Fetching sitemap from {}", base_url);

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
///
/// # Examples
///
/// ```
/// use rust_scraper::application::crawler_service::parse_sitemap;
///
/// let xml = r#"<?xml version="1.0"?>
/// <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
///     <url><loc>https://example.com/page1</loc></url>
///     <url><loc>https://example.com/page2</loc></url>
/// </urlset>"#;
///
/// let urls = parse_sitemap(xml).unwrap();
/// assert_eq!(urls.len(), 2);
/// ```
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
                let url = String::from_utf8_lossy(&e).trim().to_string();
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
    async fn test_discover_urls_invalid_url() {
        let seed = Url::parse("https://example.com").unwrap();
        let config = CrawlerConfig::new(seed);

        let result = discover_urls("not-a-valid-url", 0, &config).await;
        assert!(result.is_err());
    }
}
