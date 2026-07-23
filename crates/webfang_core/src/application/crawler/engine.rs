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

use tracing::{debug, info, instrument, span, warn, Instrument, Level};
use url::Url;

use super::collector::{CrawlMessage, ResultsCollector};
use super::concurrency_level::{ConcurrencyLevel, SharedConcurrencyLevel};
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::pipeline::{OutputStage, PipelineExecutor, ScrapedItem, StageOutcome};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::application::url_filter::is_allowed;
use crate::domain::clock::SystemClock;
use crate::domain::{CrawlError, CrawlResult, CrawlerConfig, DiscoveredUrl, JsStrategy};
use crate::infrastructure::checkpoint::store::BannedDomain;
use crate::infrastructure::checkpoint::BincodeCheckpoint;
use crate::infrastructure::crawler::robots_utils::{
    is_allowed_by_robots, new_robots_cache, RobotsCache,
};
use crate::infrastructure::crawler::{
    extract_links, fetch_url, is_internal_link, UrlQueue, UrlSource,
};
use crate::infrastructure::downloader::chromiumoxide_downloader::ChromiumoxideDownloader;
use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
use crate::infrastructure::downloader::hybrid_router::HybridRouter;
use crate::infrastructure::downloader::obscura_downloader::ObscuraDownloader;
use crate::infrastructure::downloader::resource_governor::ResourceGovernor;
use crate::infrastructure::downloader::wreq_downloader::WreqDownloader;
use crate::infrastructure::downloader::{DownloadError, Downloader, FetchedPage};
use crate::infrastructure::network::session_pool::{
    DomainSessionPool, SessionManager, SessionPoolConfig,
};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    update_engine_concurrency, ENGINE_CHECKPOINT_SAVES, ENGINE_PAGES_CRAWLED,
};

/// Shared shutdown signal — set to `true` when SIGINT/SIGTERM received.
type ShutdownSignal = Arc<AtomicBool>;

/// Type-erased fetch router that dispatches to the appropriate downloader
/// based on the configured [`JsStrategy`].
///
/// Since the `Downloader` trait uses native `async fn` in traits (not dyn-compatible),
/// we use an enum to dispatch at runtime. Inner types are `Arc`-wrapped so the router
/// can be cheaply cloned into spawned tasks.
#[derive(Clone)]
pub(crate) enum FetchRouter {
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
    /// Optional item pipeline for processing scraped content.
    pipeline: Option<Arc<PipelineExecutor>>,
    /// Output stages that receive items after pipeline processing.
    output_stages: Vec<Arc<Box<dyn OutputStage>>>,
    /// Optional autoscale level for RAM-aware concurrency adjustment.
    autoscale_level: Option<Arc<SharedConcurrencyLevel>>,
    /// Handle for the signal handler task — aborted on shutdown
    /// to prevent the tokio runtime from hanging waiting for it.
    signal_handle: Option<tokio::task::JoinHandle<()>>,
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
            pipeline: None,
            output_stages: Vec::new(),
            autoscale_level: None,
            signal_handle: None,
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
    pub fn with_session_pool(mut self, cooldown: Duration) -> Self {
        let config = SessionPoolConfig {
            base_delay: cooldown,
            ..SessionPoolConfig::default()
        };
        self.session_pool = Some(DomainSessionPool::new(config, Arc::new(SystemClock)));
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

    /// Enable autoscaled concurrency based on system RAM.
    ///
    /// Spawns a background task that polls `ResourceGovernor::ram_usage_percent()`
    /// every 5 seconds and adjusts the shared concurrency level accordingly.
    /// The engine's spawn loop reads this level to compute effective concurrency.
    pub fn with_autoscale(mut self) -> Self {
        let level = Arc::new(SharedConcurrencyLevel::new());
        let level_clone = Arc::clone(&level);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;
                let usage = ResourceGovernor::ram_usage_percent();
                let new_level = if usage >= 90 {
                    ConcurrencyLevel::Critical
                } else if usage >= 80 {
                    ConcurrencyLevel::Reduced
                } else {
                    ConcurrencyLevel::Normal
                };
                if level_clone.get() != new_level {
                    info!(
                        "Autoscale: RAM {usage}% → concurrency level {:?}",
                        new_level
                    );
                    level_clone.set(new_level);

                    #[cfg(feature = "otel-metrics")]
                    update_engine_concurrency(level_clone.get() as u64);
                }
            }
        });

        self.autoscale_level = Some(level);
        self
    }

    /// Restore banned domains from a checkpoint.
    pub fn with_banned_domains(self, domains: Vec<BannedDomain>) -> Self {
        if let Ok(mut banned) = self.banned_domains.write() {
            *banned = domains;
        }
        self
    }

    /// Set the item pipeline executor for processing scraped content.
    pub fn with_pipeline(mut self, executor: PipelineExecutor) -> Self {
        self.pipeline = Some(Arc::new(executor));
        self
    }

    /// Add an output stage that receives items after pipeline processing.
    pub fn add_output_stage(&mut self, stage: Box<dyn OutputStage>) {
        self.output_stages.push(Arc::from(stage));
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
            let _ = tokio::task::spawn_blocking(move || new_cp.save(&path))
                .in_current_span()
                .await;

            #[cfg(feature = "otel-metrics")]
            ENGINE_CHECKPOINT_SAVES.add(1, &[]);
        }
    }

    /// Spawn a signal handler that sets the shutdown flag on SIGINT/SIGTERM.
    fn spawn_signal_handler(shutdown: ShutdownSignal) -> tokio::task::JoinHandle<()> {
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
        })
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
        self.signal_handle = Some(Self::spawn_signal_handler(Arc::clone(&self.shutdown)));

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

        // Build shared task context once — all spawned tasks share this Arc
        let task_ctx = Arc::new(CrawlTaskCtx {
            config: Arc::clone(&self.config),
            visited: Arc::clone(&self.visited),
            visited_urls: Arc::clone(&self.visited_urls),
            queue: Arc::clone(&self.queue),
            rate_limiter: self.rate_limiter.clone(),
            session_pool: self.session_pool.clone(),
            ignore_robots: self.ignore_robots,
            robots_cache: self.robots_cache.clone(),
            error_count: Arc::clone(&self.error_count),
            pages_crawled: Arc::clone(&self.pages_crawled),
            collector: self.collector.as_ref().unwrap().clone(),
            cookie_bridge: Arc::clone(&self.cookie_bridge),
            banned_domains: Arc::clone(&self.banned_domains),
            fetch_router: self.fetch_router.clone(),
            pipeline: self.pipeline.clone(),
            output_stages: self.output_stages.to_vec(),
        });

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
                if pages > 0 && pages.is_multiple_of(self.checkpoint_interval) {
                    debug!("Periodic checkpoint save at {pages} pages");
                    self.save_checkpoint().await;
                }
            }

            // Spawn new tasks up to concurrency limit
            while let Some(discovered_url) = url_queue.pop_front() {
                // Check concurrency limit (autoscale-aware)
                let max_concurrent = self
                    .autoscale_level
                    .as_ref()
                    .map(|l| l.effective_concurrency(config_clone.concurrency))
                    .unwrap_or(config_clone.concurrency);
                if tasks.len() >= max_concurrent {
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

                // Spawn task — single Arc clone instead of 18 individual clones
                let task_ctx = Arc::clone(&task_ctx);
                let discovered_url_task = discovered_url.clone();
                tasks.spawn(
                    async move { run_crawl_task(task_ctx, discovered_url_task).await }
                        .in_current_span(),
                );
            }

            // If no tasks can be spawned and queue is not empty, wait for one task
            let max_concurrent = self
                .autoscale_level
                .as_ref()
                .map(|l| l.effective_concurrency(config_clone.concurrency))
                .unwrap_or(config_clone.concurrency);
            if tasks.len() >= max_concurrent && !url_queue.is_empty() {
                if let Some(result) = tasks.join_next().await {
                    handle_crawl_result(result, &self.error_count);
                }
            }
        }

        // Wait for remaining tasks
        while let Some(result) = tasks.join_next().await {
            handle_crawl_result(result, &self.error_count);
        }

        // Drop task_ctx so all cloned Senders inside CrawlTaskCtx are released.
        // Without this, collect() hangs forever — the mpsc channel stays open
        // because a Sender clone inside the Arc<CrawlTaskCtx> is still alive.
        drop(task_ctx);

        // Final checkpoint save
        self.save_checkpoint().await;

        // Collect results via mpsc channel — now all Senders are dropped,
        // so the receiver worker will drain and terminate.
        let collected_urls = self.collector.take().unwrap().collect().await;
        let total_pages = collected_urls.len();
        let errors = self.error_count.load(std::sync::atomic::Ordering::SeqCst);

        info!("Crawl complete: {} pages, {} errors", total_pages, errors);

        Ok(CrawlResult::new(collected_urls, total_pages, errors))
    }

    /// Graceful shutdown — drop the collector sender, receiver drains remaining items
    pub async fn shutdown(mut self) {
        // Abort signal handler to prevent the runtime from hanging
        if let Some(handle) = self.signal_handle.take() {
            handle.abort();
        }

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

/// Execute a single crawl task using shared context.
///
/// Extracted from the inline async block in `Engine::run()` to reduce
/// the per-spawn clone surface from 18 individual `Arc::clone()` calls
/// to a single `Arc<CrawlTaskCtx>` clone.
async fn run_crawl_task(
    ctx: Arc<CrawlTaskCtx>,
    discovered_url: DiscoveredUrl,
) -> Result<(), CrawlError> {
    // Rate limiting
    ctx.rate_limiter.until_ready().await;

    let url_str = discovered_url.url.as_str().to_string();
    let url_depth = discovered_url.depth;
    let parent_url = discovered_url.url.clone();

    // Session pool: check if domain is healthy before fetching
    let mut session_id = None;
    if let Some(ref pool) = ctx.session_pool {
        let domain = url::Url::parse(&url_str)
            .ok()
            .and_then(|u| u.host_str().map(String::from))
            .unwrap_or_default();
        match pool.acquire(&domain) {
            Some(id) => {
                session_id = Some(id);
            },
            None => {
                debug!("Domain {} has no available sessions, skipping", domain);
                return Ok(());
            },
        }
    }

    debug!("Crawling: {} (depth={})", url_str, url_depth);

    // Fetch URL — use fetch_router if available, else static fetch_url()
    let (response, fetched_cookies) = if let Some(ref router) = ctx.fetch_router {
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
                    if let Ok(mut domains) = ctx.banned_domains.write() {
                        if !domains.iter().any(|d| d.domain == domain) {
                            domains.push(banned);
                            warn!("Banned domain {} due to WAF: {}", domain, msg);
                        }
                    }
                }
                return Err(DownloadError::WafChallenge(msg).into());
            },
            Err(e) => {
                return Err(e.into());
            },
        }
    } else {
        match fetch_url(&url_str, &ctx.config).await {
            Ok(html) => (html, Vec::new()),
            Err(e) => {
                if format!("{e}").contains("WAF") {
                    // Ban the domain
                    if let Ok(parsed) = url::Url::parse(&url_str) {
                        if let Some(domain) = parsed.host_str() {
                            let banned = BannedDomain {
                                domain: domain.to_string(),
                                banned_until: None,
                                reason: e.to_string(),
                            };
                            if let Ok(mut domains) = ctx.banned_domains.write() {
                                if !domains.iter().any(|d| d.domain == domain) {
                                    domains.push(banned);
                                    warn!("Banned domain {} due to WAF: {}", domain, e);
                                }
                            }
                        }
                    }
                }
                return Err(e);
            },
        }
    };

    // Ingest cookies into the cookie bridge
    if !fetched_cookies.is_empty() {
        if let Ok(mut bridge) = ctx.cookie_bridge.write() {
            for cookie in &fetched_cookies {
                bridge.add(cookie.clone());
            }
        }
    }

    // Report success to session pool
    if let Some(ref pool) = ctx.session_pool {
        if let Some(id) = session_id {
            if let Ok(parsed) = url::Url::parse(&url_str) {
                if let Some(domain) = parsed.host_str() {
                    pool.report_success(domain, id);
                }
            }
        }
    }

    // Track pages crawled
    ctx.pages_crawled
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    #[cfg(feature = "otel-metrics")]
    ENGINE_PAGES_CRAWLED.add(1, &[]);

    // Pipeline processing: convert to ScrapedItem and run through pipeline
    if let Some(ref pipeline) = ctx.pipeline {
        let item = ScrapedItem {
            url: url_str.clone(),
            raw_html: response.clone(),
            text_content: None,
            metadata: std::collections::HashMap::new(),
            status_code: 200,
            embeddings: None,
        };

        match pipeline.execute(item).await {
            StageOutcome::Continue(processed_item) => {
                // Pass to output stages
                for stage in &ctx.output_stages {
                    if let Err(e) = stage.write(&processed_item).await {
                        warn!("Output stage '{}' failed: {}", stage.name(), e);
                    }
                }
            },
            StageOutcome::Skip => {
                debug!("Pipeline skipped item: {}", url_str);
                return Ok(());
            },
            StageOutcome::Reject(reason) => {
                warn!("Pipeline rejected {}: {}", url_str, reason);
                return Ok(());
            },
        }
    }

    // Add to results via channel (sin lock)
    if let Err(e) = ctx
        .collector
        .send(CrawlMessage::success(discovered_url))
        .await
    {
        debug!("Failed to send result: {}", e);
    }

    // Extract links and add to queue
    if url_depth < ctx.config.max_depth {
        match extract_links(&response, &url_str) {
            Ok(links) => {
                for link in links {
                    // extract_links() already normalizes each link
                    if let Ok(parsed_url) = Url::parse(&link) {
                        if let Some(seed_domain) = ctx.config.seed_url.host_str() {
                            let link_domain = parsed_url.host_str().unwrap_or("");
                            if is_internal_link(&link, seed_domain)
                                && is_allowed(&link, &ctx.config)
                                && (ctx.ignore_robots
                                    || is_allowed_by_robots(&link, link_domain, &ctx.robots_cache)
                                        .await)
                                && ctx.visited.try_insert(&link)
                            {
                                // Record URL string for checkpoint
                                if let Ok(mut urls) = ctx.visited_urls.write() {
                                    urls.push(link.clone());
                                }

                                let new_discovered = DiscoveredUrl::html(
                                    parsed_url,
                                    url_depth + 1,
                                    parent_url.clone(),
                                );
                                ctx.queue
                                    .push_prioritized(new_discovered, UrlSource::Link)
                                    .await;
                            }
                        }
                    }
                }
            },
            Err(e) => {
                warn!("Failed to extract links from {}: {}", url_str, e);
                ctx.error_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            },
        }
    }

    Ok(())
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
    /// Enable autoscaled concurrency based on system RAM.
    pub autoscale_enabled: bool,
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
/// use webfang::{domain::CrawlerConfig, application::crawl_site};
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
/// use webfang::{domain::CrawlerConfig, application::crawl_site_with_options};
/// use webfang::application::crawler::engine::EngineOptions;
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
///     autoscale_enabled: true,
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
        engine = engine.with_session_pool(Duration::from_secs(2));
    }

    // Apply JS strategy
    engine = engine.with_js_strategy(options.js_strategy);

    // Apply autoscale if enabled
    if options.autoscale_enabled {
        engine = engine.with_autoscale();
    }

    let result = engine.run().await;
    engine.shutdown().await;
    result
}

#[cfg(test)]
#[cfg(feature = "otel-metrics")]
mod metrics_tests {
    use crate::infrastructure::observability::metrics_instruments::{
        engine_concurrency_get, update_engine_concurrency, ENGINE_CHECKPOINT_SAVES,
        ENGINE_CONCURRENCY_LEVEL, ENGINE_PAGES_CRAWLED,
    };

    #[test]
    fn test_engine_pages_crawled_instrument_init() {
        let _ = &*ENGINE_PAGES_CRAWLED;
    }

    #[test]
    fn test_engine_checkpoint_saves_instrument_init() {
        let _ = &*ENGINE_CHECKPOINT_SAVES;
    }

    #[test]
    fn test_update_engine_concurrency_init() {
        // Should not panic — setting gauge value
        update_engine_concurrency(5);
        let val = engine_concurrency_get();
        assert_eq!(val, 5);
    }

    #[test]
    fn test_engine_concurrency_level_gauge_init() {
        let _ = &*ENGINE_CONCURRENCY_LEVEL;
    }
}
