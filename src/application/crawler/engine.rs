//! Engine module — Crawl orchestration with JoinSet-based concurrency
//!
//! The Engine manages the crawl loop, spawning tasks via JoinSet
//! with backpressure and rate limiting. Each task fetches a URL,
//! extracts links, and pushes discovered URLs to the queue.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;

use tracing::{debug, error, info, instrument, span, warn, Level};
use url::Url;

use crate::application::deduplicator::UrlDeduplicator;
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::application::results_channel::{CrawlMessage, ResultsCollector};
use crate::application::url_filter::is_allowed;
use crate::domain::{CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl};
use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, normalize_url, UrlQueue,
};

/// Crawl engine — orchestrates URL fetching with concurrency control
///
/// Uses `JoinSet` for task management (no redundant Semaphore).
/// Rate limiting via `SharedRateLimiter`. Deduplication via lock-free
/// `UrlDeduplicator`. Results collected via mpsc channel.
pub struct Engine {
    config: Arc<CrawlerConfig>,
    collector: Option<ResultsCollector>,
    visited: Arc<UrlDeduplicator>,
    queue: Arc<UrlQueue>,
    rate_limiter: SharedRateLimiter,
    error_count: Arc<AtomicUsize>,
}

impl Engine {
    /// Create a new Engine from a CrawlerConfig
    fn new(config: CrawlerConfig) -> Result<Self, CrawlError> {
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

        // Track visited URLs — lock-free DashSet
        let visited = Arc::new(UrlDeduplicator::new());

        // Results collector via mpsc channel
        let collector = ResultsCollector::new(config_clone.max_pages, Some(config_clone.max_pages));
        let error_count = Arc::new(AtomicUsize::new(0));

        Ok(Self {
            config,
            collector: Some(collector),
            visited,
            queue,
            rate_limiter,
            error_count,
        })
    }

    /// Run the crawl loop until completion
    ///
    /// Returns the collected URLs and error count.
    pub async fn run(&mut self) -> Result<CrawlResult, CrawlError> {
        let config_clone = Arc::clone(&self.config);

        // Add seed URL to queue
        let seed_discovered = DiscoveredUrl::html(
            config_clone.seed_url.clone(),
            0,
            config_clone.seed_url.clone(),
        );
        self.queue.push(seed_discovered).await;

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
            if self.collector.as_ref().unwrap().is_full(config_clone.max_pages) {
                info!("Reached max pages limit: {}", config_clone.max_pages);
                break;
            }

            // Process completed tasks FIRST (non-blocking)
            while let Some(result) = tasks.try_join_next() {
                handle_crawl_result(result, &self.error_count);
            }

            // Drain discovered links from the deduplicated UrlQueue
            url_queue.append(&mut self.queue.drain_all().await);

            // Spawn new tasks up to concurrency limit
            while let Some(discovered_url) = url_queue.pop_front() {
                // Check concurrency limit
                if tasks.len() >= config_clone.concurrency {
                    url_queue.push_front(discovered_url);
                    break;
                }

                // Check if already visited — atomic, lock-free
                if !self.visited.try_insert(discovered_url.url.as_str()) {
                    continue;
                }

                // Clone data for task (async-clone-before-await)
                let config_task = Arc::clone(&self.config);
                let queue_task = Arc::clone(&self.queue);
                let results_sender = self.collector.as_ref().unwrap().clone();
                let visited_task = Arc::clone(&self.visited);
                let error_count_task = Arc::clone(&self.error_count);
                let rate_limiter_task = self.rate_limiter.clone();
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
                                                if let Some(seed_domain) =
                                                    config_task.seed_url.host_str()
                                                {
                                                    if is_internal_link(&normalized, seed_domain) {
                                                        if is_allowed(&normalized, &config_task) {
                                                            if visited_task.try_insert(&normalized)
                                                            {
                                                                let new_discovered =
                                                                    DiscoveredUrl::html(
                                                                        parsed_url,
                                                                        url_depth + 1,
                                                                        parent_url.clone(),
                                                                    );
                                                                queue_task
                                                                    .push(new_discovered)
                                                                    .await;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    },
                                    Err(e) => {
                                        warn!("Failed to extract links from {}: {}", url_str, e);
                                        error_count_task.fetch_add(
                                            1,
                                            std::sync::atomic::Ordering::SeqCst,
                                        );
                                    },
                                }
                            }
                        },
                        Err(e) => {
                            error!("Failed to fetch {}: {}", url_str, e);
                            error_count_task
                                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            return Err(e);
                        },
                    }

                    Ok(())
                });
            }

            // If no tasks can be spawned and queue is not empty, wait for one task
            if tasks.len() >= config_clone.concurrency && !url_queue.is_empty() {
                if let Some(result) = tasks.join_next().await {
                    handle_crawl_result(result, &self.error_count);
                }
            }
        }

        // Wait for remaining tasks
        while let Some(result) = tasks.join_next().await {
            handle_crawl_result(result, &self.error_count);
        }

        // Collect results via mpsc channel (shutdown limpio)
        let collected_urls = self.collector.take().unwrap().collect().await;
        let total_pages = collected_urls.len();
        let errors = self.error_count.load(std::sync::atomic::Ordering::SeqCst);

        info!("Crawl complete: {} pages, {} errors", total_pages, errors);

        Ok(CrawlResult::new(collected_urls, total_pages, errors))
    }

    /// Graceful shutdown — drop the collector sender, receiver drains remaining items
    pub async fn shutdown(mut self) {
        // Take the collector to drop the sender — receiver will drain remaining items
        // The JoinSet tasks will complete naturally
        self.collector.take();
        info!("Engine shutdown complete");
    }
}

/// Handle result from a completed crawl task
fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
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

/// Crawl a website starting from the seed URL
///
/// Thin wrapper that creates an Engine, runs the crawl loop, and shuts down.
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

    let mut engine = Engine::new(config)?;
    let result = engine.run().await;
    engine.shutdown().await;
    result
}
