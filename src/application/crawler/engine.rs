//! Engine module — Crawl orchestration with JoinSet-based concurrency
//!
//! The Engine manages the crawl loop, spawning tasks via JoinSet
//! with backpressure and rate limiting. Each task fetches a URL,
//! extracts links, and pushes discovered URLs to the queue.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tracing::{debug, info, instrument, span, warn, Level};
use url::Url;

use super::collector::{CrawlMessage, ResultsCollector};
use super::discovery::{is_allowed_by_robots, new_robots_cache, RobotsCache};
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::application::url_filter::is_allowed;
use crate::domain::{CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl, JsStrategy};
use crate::infrastructure::checkpoint::store::BannedDomain;
use crate::infrastructure::checkpoint::BincodeCheckpoint;
use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, normalize_url, UrlQueue, UrlSource,
};
use crate::infrastructure::downloader::chromiumoxide_downloader::ChromiumoxideDownloader;
use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
use crate::infrastructure::downloader::hybrid_router::HybridRouter;
use crate::infrastructure::downloader::obscura_downloader::ObscuraDownloader;
use crate::infrastructure::downloader::wreq_downloader::WreqDownloader;
use crate::infrastructure::downloader::{DownloadError, Downloader, FetchedPage};
use crate::infrastructure::session::DomainSessionPool;

/// Shared shutdown signal — set to `true` when SIGINT/SIGTERM received.
type ShutdownSignal = Arc<AtomicBool>;

/// Type-erased fetch router that dispatches to the appropriate downloader
/// based on the configured [`JsStrategy`].
///
/// Since the `Downloader` trait uses native `async fn` in traits (not dyn-compatible),
/// we use an enum to dispatch at runtime. Inner types are `Arc`-wrapped so the router
/// can be cheaply cloned into spawned tasks.
#[derive(Clone)]
enum FetchRouter {
    /// Static HTTP only (wreq). Default.
    Static(Arc<WreqDownloader>),
    /// Hybrid 3-layer: wreq → Obscura → Chromiumoxide.
    Hybrid(Arc<HybridRouter<WreqDownloader, ObscuraDownloader, ChromiumoxideDownloader>>),
}

impl FetchRouter {
    async fn fetch(
        &self,
        url: &Url,
    ) -> Result<FetchedPage, crate::infrastructure::downloader::DownloadError> {
        match self {
            Self::Static(dl) => dl.fetch(url).await,
            Self::Hybrid(dl) => dl.fetch(url).await,
        }
    }

    #[allow(dead_code)]
    fn supports_interactions(&self) -> bool {
        match self {
            Self::Static(dl) => dl.supports_interactions(),
            Self::Hybrid(dl) => dl.supports_interactions(),
        }
    }
}

/// Crawl engine — orchestrates URL fetching with concurrency control
///
/// Uses `JoinSet` for task management (no redundant Semaphore).
/// Rate limiting via `SharedRateLimiter`. Deduplication via lock-free
/// `UrlDeduplicator`. Results collected via mpsc channel.
pub struct Engine {
    config: Arc<CrawlerConfig>,
    collector: Option<ResultsCollector>,
    visited: Arc<UrlDeduplicator>,
    /// String URLs for checkpoint persistence (mirrors `visited` hashes).
    visited_urls: Arc<RwLock<Vec<String>>>,
    queue: Arc<UrlQueue>,
    rate_limiter: SharedRateLimiter,
    error_count: Arc<AtomicUsize>,
    /// Optional checkpoint persistence for crash recovery.
    checkpoint: Option<BincodeCheckpoint>,
    /// Path to save checkpoint files.
    checkpoint_path: Option<PathBuf>,
    /// Pages between automatic checkpoint saves (0 = disabled).
    checkpoint_interval: u64,
    /// Skip robots.txt enforcement.
    ignore_robots: bool,
    /// Shared robots.txt cache for the crawl session.
    robots_cache: RobotsCache,
    /// Optional domain session pool for per-domain rate limiting.
    session_pool: Option<DomainSessionPool>,
    /// Atomic counter for total pages crawled (used by checkpoint and signal handler).
    pages_crawled: Arc<AtomicU64>,
    /// Shared shutdown signal for graceful termination.
    shutdown: ShutdownSignal,
    /// JavaScript rendering strategy.
    js_strategy: JsStrategy,
    /// Optional fetch router for hybrid/full JS rendering.
    fetch_router: Option<FetchRouter>,
    /// Cookie bridge for extracting and injecting cookies.
    cookie_bridge: Arc<RwLock<CookieBridge>>,
    /// Domains currently banned due to WAF or rate limiting.
    banned_domains: Arc<RwLock<Vec<BannedDomain>>>,
}

impl Engine {
    /// Create a new Engine from a CrawlerConfig
    fn new(config: CrawlerConfig, ignore_robots: bool) -> Result<Self, CrawlError> {
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

        // Track visited URLs — lock-free DashSet for dedup, RwLock Vec for checkpoint
        let visited = Arc::new(UrlDeduplicator::new());
        let visited_urls = Arc::new(RwLock::new(Vec::new()));

        // Results collector via mpsc channel
        let collector = ResultsCollector::new(config_clone.max_pages, Some(config_clone.max_pages));
        let error_count = Arc::new(AtomicUsize::new(0));
        let pages_crawled = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        Ok(Self {
            config,
            collector: Some(collector),
            visited,
            visited_urls,
            queue,
            rate_limiter,
            error_count,
            checkpoint: None,
            checkpoint_path: None,
            checkpoint_interval: 100,
            ignore_robots,
            robots_cache: new_robots_cache(),
            session_pool: None,
            pages_crawled,
            shutdown,
            js_strategy: JsStrategy::default(),
            fetch_router: None,
            cookie_bridge: Arc::new(RwLock::new(CookieBridge::new())),
            banned_domains: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Enable checkpoint persistence with the given interval and base directory.
    pub fn with_checkpoint(mut self, interval: u64, base_dir: PathBuf) -> Self {
        use crate::infrastructure::checkpoint::store::CheckpointPath;
        let cp_path = CheckpointPath::new(&base_dir);
        cp_path.ensure_dir().unwrap_or_else(|e| {
            warn!("Failed to create checkpoint dir: {e}");
        });

        match BincodeCheckpoint::load(&cp_path.file()) {
            Ok(cp) => {
                info!(
                    "Resuming from checkpoint: {} visited, {} pages",
                    cp.visited.len(),
                    cp.pages_crawled
                );
                self.checkpoint = Some(cp);
            },
            Err(e) => {
                warn!("Failed to load checkpoint, starting fresh: {e}");
                self.checkpoint = Some(BincodeCheckpoint::default());
            },
        }

        self.checkpoint_path = Some(cp_path.file());
        self.checkpoint_interval = interval;
        self
    }

    /// Enable the domain session pool for per-domain rate limiting.
    pub fn with_session_pool(mut self, cooldown: Duration, max_failures: u32) -> Self {
        self.session_pool = Some(DomainSessionPool::new(cooldown, max_failures));
        self
    }

    /// Set the JavaScript rendering strategy.
    pub fn with_js_strategy(mut self, strategy: JsStrategy) -> Self {
        self.js_strategy = strategy;
        match strategy {
            JsStrategy::Static => {
                self.fetch_router =
                    Some(FetchRouter::Static(Arc::new(WreqDownloader::new(30, 10))));
            },
            JsStrategy::Hybrid => {
                let l1 = WreqDownloader::new(30, 10);
                let l2 = ObscuraDownloader::new();
                let l3 = ChromiumoxideDownloader::new();
                self.fetch_router =
                    Some(FetchRouter::Hybrid(Arc::new(HybridRouter::new(l1, l2, l3))));
            },
            JsStrategy::Full => {
                // Full strategy: use Chromiumoxide only via HybridRouter with wreq fallback
                let l1 = WreqDownloader::new(30, 10);
                let l2 = ObscuraDownloader::new();
                let l3 = ChromiumoxideDownloader::new();
                self.fetch_router =
                    Some(FetchRouter::Hybrid(Arc::new(HybridRouter::new(l1, l2, l3))));
            },
        }
        self
    }

    /// Restore banned domains from a checkpoint.
    pub fn with_banned_domains(self, domains: Vec<BannedDomain>) -> Self {
        if let Ok(mut banned) = self.banned_domains.write() {
            *banned = domains;
        }
        self
    }

    /// Save the current checkpoint to disk (non-blocking wrapper).
    async fn save_checkpoint(&self) {
        if let (Some(_cp), Some(path)) = (&self.checkpoint, &self.checkpoint_path) {
            let visited_set: HashSet<String> = {
                let urls = self.visited_urls.read().unwrap();
                urls.iter().cloned().collect()
            };
            let pages = self
                .pages_crawled
                .load(std::sync::atomic::Ordering::Relaxed);
            let banned = self
                .banned_domains
                .read()
                .map(|d| d.clone())
                .unwrap_or_default();
            let new_cp = BincodeCheckpoint::from_state(&visited_set, &[], pages, banned);

            // Save on blocking thread to avoid blocking the event loop
            let path = path.clone();
            let _ = tokio::task::spawn_blocking(move || new_cp.save(&path)).await;
        }
    }

    /// Spawn a signal handler that sets the shutdown flag on SIGINT/SIGTERM.
    fn spawn_signal_handler(shutdown: ShutdownSignal) {
        tokio::spawn(async move {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm =
                    signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
                tokio::select! {
                    _ = ctrl_c => {
                        info!("Received SIGINT — initiating graceful shutdown");
                    },
                    _ = sigterm.recv() => {
                        info!("Received SIGTERM — initiating graceful shutdown");
                    },
                }
            }
            #[cfg(not(unix))]
            {
                ctrl_c.await.ok();
                info!("Received interrupt — initiating graceful shutdown");
            }
            shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }

    /// Record a URL as visited (both hash dedup and string tracking).
    fn record_visit(&self, url: &str) -> bool {
        if self.visited.try_insert(url) {
            if let Ok(mut urls) = self.visited_urls.write() {
                urls.push(url.to_string());
            }
            true
        } else {
            false
        }
    }

    /// Run the crawl loop until completion
    ///
    /// Returns the collected URLs and error count.
    pub async fn run(&mut self) -> Result<CrawlResult, CrawlError> {
        let config_clone = Arc::clone(&self.config);

        // Spawn signal handler for graceful shutdown
        Self::spawn_signal_handler(Arc::clone(&self.shutdown));

        // Load checkpoint state if resuming
        if let Some(ref cp) = self.checkpoint {
            if !cp.visited.is_empty() {
                for url in &cp.visited {
                    self.record_visit(url);
                }
                info!("Restored {} visited URLs from checkpoint", cp.visited.len());
            }
            if !cp.banned_domains.is_empty() {
                if let Ok(mut banned) = self.banned_domains.write() {
                    *banned = cp.banned_domains.clone();
                }
                info!(
                    "Restored {} banned domains from checkpoint",
                    cp.banned_domains.len()
                );
            }
        }

        // Add seed URL to queue (highest priority)
        let seed_discovered = DiscoveredUrl::html(
            config_clone.seed_url.clone(),
            0,
            config_clone.seed_url.clone(),
        );
        self.queue
            .push_prioritized(seed_discovered, UrlSource::Seed)
            .await;

        let mut tasks = tokio::task::JoinSet::new();
        let mut url_queue = std::collections::VecDeque::new();
        url_queue.push_back(DiscoveredUrl::html(
            config_clone.seed_url.clone(),
            0,
            config_clone.seed_url.clone(),
        ));

        // Main crawl loop
        while !url_queue.is_empty() || !tasks.is_empty() {
            // Check shutdown signal
            if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Shutdown signal received — saving checkpoint and exiting");
                self.save_checkpoint().await;
                break;
            }

            // Check if we've reached max pages (sin lock - atomic)
            if self
                .collector
                .as_ref()
                .unwrap()
                .is_full(config_clone.max_pages)
            {
                info!("Reached max pages limit: {}", config_clone.max_pages);
                break;
            }

            // Process completed tasks FIRST (non-blocking)
            while let Some(result) = tasks.try_join_next() {
                handle_crawl_result(result, &self.error_count);
            }

            // Drain discovered links from the deduplicated UrlQueue
            url_queue.append(&mut self.queue.drain_all().await);

            // Periodic checkpoint save
            if self.checkpoint_interval > 0 {
                let pages = self
                    .pages_crawled
                    .load(std::sync::atomic::Ordering::Relaxed);
                if pages > 0 && pages % self.checkpoint_interval == 0 {
                    debug!("Periodic checkpoint save at {pages} pages");
                    self.save_checkpoint().await;
                }
            }

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
                // Record URL string for checkpoint (we just inserted into hash set)
                if let Ok(mut urls) = self.visited_urls.write() {
                    urls.push(discovered_url.url.as_str().to_string());
                }

                // Clone data for task (async-clone-before-await)
                let config_task = Arc::clone(&self.config);
                let queue_task = Arc::clone(&self.queue);
                let results_sender = self.collector.as_ref().unwrap().clone();
                let visited_task = Arc::clone(&self.visited);
                let visited_urls_task = Arc::clone(&self.visited_urls);
                let error_count_task = Arc::clone(&self.error_count);
                let rate_limiter_task = self.rate_limiter.clone();
                let discovered_url_task = discovered_url.clone();
                let session_pool_task = self.session_pool.clone();
                let pages_crawled_task = Arc::clone(&self.pages_crawled);
                let ignore_robots_task = self.ignore_robots;
                let robots_cache_task = self.robots_cache.clone();
                let cookie_bridge_task = Arc::clone(&self.cookie_bridge);
                let banned_domains_task = Arc::clone(&self.banned_domains);
                let fetch_router_task = self.fetch_router.clone();

                // Clone parent URL before moving discovered_url_task
                let parent_url = discovered_url_task.url.clone();

                // Spawn task
                tasks.spawn(async move {
                    // Rate limiting
                    rate_limiter_task.until_ready().await;

                    let url_str = discovered_url_task.url.as_str().to_string();
                    let url_depth = discovered_url_task.depth;

                    // Session pool: check if domain is healthy before fetching
                    if let Some(ref pool) = session_pool_task {
                        let domain = url::Url::parse(&url_str)
                            .ok()
                            .and_then(|u| u.host_str().map(String::from))
                            .unwrap_or_default();
                        match pool.acquire(&domain).await {
                            Ok(true) => {}, // proceed
                            Ok(false) => {
                                debug!("Domain {} on cooldown, skipping", domain);
                                return Ok(());
                            },
                            Err(e) => {
                                warn!("Domain {} unhealthy: {e}", domain);
                                return Ok(());
                            },
                        }
                    }

                    debug!("Crawling: {} (depth={})", url_str, url_depth);

                    // Fetch URL — use fetch_router if available, else static fetch_url()
                    let (response, fetched_cookies) = if let Some(ref router) = fetch_router_task {
                        let parsed_url = url::Url::parse(&url_str)
                            .map_err(|e| CrawlError::Internal(format!("invalid URL: {e}")))?;
                        match router.fetch(&parsed_url).await {
                            Ok(page) => {
                                let cookies = page.cookies.clone();
                                (page.html, cookies)
                            },
                            Err(DownloadError::WafChallenge(msg)) => {
                                // Ban the domain
                                if let Some(domain) = parsed_url.host_str() {
                                    let banned = BannedDomain {
                                        domain: domain.to_string(),
                                        banned_until: None,
                                        reason: msg.clone(),
                                    };
                                    if let Ok(mut domains) = banned_domains_task.write() {
                                        if !domains.iter().any(|d| d.domain == domain) {
                                            domains.push(banned);
                                            warn!("Banned domain {} due to WAF: {}", domain, msg);
                                        }
                                    }
                                }
                                return Err(CrawlError::Download(format!("WAF: {msg}")));
                            },
                            Err(e) => {
                                return Err(CrawlError::Download(e.to_string()));
                            },
                        }
                    } else {
                        match fetch_url(&url_str, &config_task).await {
                            Ok(html) => (html, Vec::new()),
                            Err(CrawlError::Download(ref msg)) if msg.contains("WAF") => {
                                // Ban the domain
                                if let Ok(parsed) = url::Url::parse(&url_str) {
                                    if let Some(domain) = parsed.host_str() {
                                        let banned = BannedDomain {
                                            domain: domain.to_string(),
                                            banned_until: None,
                                            reason: msg.clone(),
                                        };
                                        if let Ok(mut domains) = banned_domains_task.write() {
                                            if !domains.iter().any(|d| d.domain == domain) {
                                                domains.push(banned);
                                                warn!(
                                                    "Banned domain {} due to WAF: {}",
                                                    domain, msg
                                                );
                                            }
                                        }
                                    }
                                }
                                return Err(CrawlError::Download(msg.clone()));
                            },
                            Err(e) => return Err(e),
                        }
                    };

                    // Ingest cookies into the cookie bridge
                    if !fetched_cookies.is_empty() {
                        if let Ok(mut bridge) = cookie_bridge_task.write() {
                            for cookie in &fetched_cookies {
                                bridge.add(cookie.clone());
                            }
                        }
                    }

                    // Report success to session pool
                    if let Some(ref pool) = session_pool_task {
                        if let Ok(parsed) = url::Url::parse(&url_str) {
                            if let Some(domain) = parsed.host_str() {
                                pool.report_success(domain).await;
                            }
                        }
                    }

                    // Track pages crawled
                    pages_crawled_task.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

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
                                        if let Some(seed_domain) = config_task.seed_url.host_str() {
                                            let link_domain = parsed_url.host_str().unwrap_or("");
                                            if is_internal_link(&normalized, seed_domain)
                                                && is_allowed(&normalized, &config_task)
                                                && (ignore_robots_task
                                                    || is_allowed_by_robots(
                                                        &normalized,
                                                        link_domain,
                                                        &robots_cache_task,
                                                    )
                                                    .await)
                                                && visited_task.try_insert(&normalized)
                                            {
                                                // Record URL string for checkpoint
                                                if let Ok(mut urls) = visited_urls_task.write() {
                                                    urls.push(normalized.clone());
                                                }

                                                let new_discovered = DiscoveredUrl::html(
                                                    parsed_url,
                                                    url_depth + 1,
                                                    parent_url.clone(),
                                                );
                                                queue_task
                                                    .push_prioritized(
                                                        new_discovered,
                                                        UrlSource::Link,
                                                    )
                                                    .await;
                                            }
                                        }
                                    }
                                }
                            },
                            Err(e) => {
                                warn!("Failed to extract links from {}: {}", url_str, e);
                                error_count_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            },
                        }
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

        // Final checkpoint save
        self.save_checkpoint().await;

        // Collect results via mpsc channel (shutdown limpio)
        let collected_urls = self.collector.take().unwrap().collect().await;
        let total_pages = collected_urls.len();
        let errors = self.error_count.load(std::sync::atomic::Ordering::SeqCst);

        info!("Crawl complete: {} pages, {} errors", total_pages, errors);

        Ok(CrawlResult::new(collected_urls, total_pages, errors))
    }

    /// Graceful shutdown — drop the collector sender, receiver drains remaining items
    pub async fn shutdown(mut self) {
        // Save checkpoint before shutting down
        self.save_checkpoint().await;

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

/// Engine-level crawl options — controls Engine internals beyond CrawlerConfig.
///
/// While `CrawlerConfig` defines *what* to crawl (seed, depth, patterns),
/// `EngineOptions` controls *how* the Engine operates (checkpointing,
/// session pooling, robots.txt enforcement).
///
/// Named `EngineOptions` (not `CrawlOptions`) to avoid collision with
/// `application::crawl_options::CrawlOptions`, which is the CLI-level
/// configuration struct.
#[derive(Debug, Clone, Default)]
pub struct EngineOptions {
    /// Path to save checkpoint files. `None` disables checkpointing.
    pub checkpoint_path: Option<PathBuf>,
    /// Enable the domain session pool for per-domain rate limiting.
    pub session_pool_enabled: bool,
    /// Skip robots.txt enforcement.
    pub ignore_robots: bool,
    /// JavaScript rendering strategy.
    pub js_strategy: JsStrategy,
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

    let ignore_robots = config.ignore_robots;
    let mut engine = Engine::new(config, ignore_robots)?;
    let result = engine.run().await;
    engine.shutdown().await;
    result
}

/// Crawl a website with fine-grained engine options.
///
/// This is the advanced entry point for callers that need checkpointing,
/// session pooling, or explicit robots.txt control beyond what
/// `CrawlerConfig.ignore_robots` provides.
///
/// # Arguments
///
/// * `config` - Crawler configuration (seed, depth, patterns, etc.)
/// * `options` - Engine-level options (checkpoint, session pool, robots)
///
/// # Returns
///
/// * `Ok(CrawlResult)` - Crawl result with discovered URLs
/// * `Err(CrawlError)` - Error during crawling
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::{domain::CrawlerConfig, application::crawl_site_with_options};
/// use rust_scraper::application::crawler::engine::EngineOptions;
/// use std::time::Duration;
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
/// let options = EngineOptions {
///     checkpoint_path: Some(std::path::PathBuf::from("/tmp/checkpoint")),
///     session_pool_enabled: true,
///     ignore_robots: false,
/// };
///
/// let result = crawl_site_with_options(config, options).await?;
/// println!("Crawled {} pages", result.total_pages);
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "crawl_site_with_options",
    skip(config, options),
    fields(
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages,
        checkpoint_enabled = options.checkpoint_path.is_some(),
        session_pool = options.session_pool_enabled,
        ignore_robots = options.ignore_robots
    )
)]
pub async fn crawl_site_with_options(
    config: CrawlerConfig,
    options: EngineOptions,
) -> Result<CrawlResult, CrawlError> {
    let span = span!(
        Level::INFO,
        "crawl_site_with_options",
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages
    );
    let _guard = span.enter();

    info!(
        "Starting crawl from {} with max_depth={} max_pages={} (checkpoint={}, session_pool={}, ignore_robots={})",
        config.seed_url,
        config.max_depth,
        config.max_pages,
        options.checkpoint_path.is_some(),
        options.session_pool_enabled,
        options.ignore_robots
    );

    let mut engine = Engine::new(config, options.ignore_robots)?;

    // Apply checkpoint if path provided
    if let Some(ref path) = options.checkpoint_path {
        engine = engine.with_checkpoint(100, path.clone());
    }

    // Apply session pool if enabled
    if options.session_pool_enabled {
        engine = engine.with_session_pool(Duration::from_secs(2), 5);
    }

    // Apply JS strategy
    engine = engine.with_js_strategy(options.js_strategy);

    let result = engine.run().await;
    engine.shutdown().await;
    result
}
