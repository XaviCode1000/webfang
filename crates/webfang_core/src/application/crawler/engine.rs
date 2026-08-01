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
use wreq_util::Profile;

use futures::future::BoxFuture;

use super::checkpoint::{
    BannedDomain, BincodeCheckpoint, CheckpointPath, CheckpointStore, CrawlCheckpoint,
};
use super::collector::{CrawlMessage, ResultsCollector};
use super::concurrency_level::{ConcurrencyLevel, SharedConcurrencyLevel};
use super::progress::CrawlProgress;
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::application::deduplicator::UrlDeduplicator;
use crate::application::pipeline::{OutputStage, PipelineExecutor, ScrapedItem, StageOutcome};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::application::url_filter::is_allowed;
use crate::domain::clock::SystemClock;
use crate::domain::{
    CorrelationId, CrawlError, CrawlErrorCategory, CrawlResult, CrawlerConfig, DiscoveredUrl,
    JsStrategy,
};
use crate::infrastructure::crawler::robots_utils::RobotsFetcher;
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
use crate::infrastructure::observability::log_scrape_error;

/// Shared shutdown signal — set to `true` when SIGINT/SIGTERM received.
type ShutdownSignal = Arc<AtomicBool>;

/// Type-erased fetch router that dispatches to the appropriate downloader
/// based on the configured [`JsStrategy`].
///
/// Implements [`Downloader`] so it can be passed as `&dyn Downloader` for
/// runtime dispatch and test mocking. Inner types are `Arc`-wrapped so the
/// router can be cheaply cloned into spawned tasks.
#[derive(Clone)]
pub enum FetchRouter {
    /// Static HTTP only (wreq). Default.
    Static(Arc<WreqDownloader>),
    /// Hybrid 3-layer: wreq → Obscura → Chromiumoxide.
    Hybrid(Arc<HybridRouter<WreqDownloader, ObscuraDownloader, ChromiumoxideDownloader>>),
    /// Chrome-direct rendering with RAM-aware concurrency gating.
    ///
    /// Bypasses the wreq → Obscura escalation and renders every page in
    /// Chromiumoxide. A dedicated [`ResourceGovernor`] checks system memory
    /// before each fetch and holds a semaphore permit for its duration,
    /// preventing OOM on large crawls. Both fields are `Arc`-wrapped so the
    /// router stays cheaply cloneable into spawned tasks.
    Full(Arc<ChromiumoxideDownloader>, Arc<ResourceGovernor>),
}

/// Build the [`FetchRouter`] for a given JavaScript rendering strategy.
///
/// Single source of truth for the strategy → router mapping, shared by the
/// crawl [`Engine`] and the CLI scrape path. `timeout_secs` drives the wreq
/// request timeout (connect timeout is clamped to 10s); `tls_emulation` is the
/// TLS/HTTP2 fingerprint profile applied to the wreq layer; `cookie_bridge` is
/// shared with the Chromiumoxide layer for cookie injection. `ignore_waf`
/// bypasses WAF classification on the hybrid spa-detection path (REQ-WAF-07).
///
/// # Errors
///
/// Returns [`DownloadError::Internal`] if the wreq client cannot be built.
pub fn build_fetch_router(
    strategy: &JsStrategy,
    timeout_secs: u64,
    tls_emulation: Profile,
    cookie_bridge: Arc<RwLock<CookieBridge>>,
    ignore_waf: bool,
) -> Result<FetchRouter, DownloadError> {
    let connect_timeout = timeout_secs.min(10);
    Ok(match strategy {
        JsStrategy::Static => FetchRouter::Static(Arc::new(WreqDownloader::new(
            timeout_secs,
            connect_timeout,
            tls_emulation,
        )?)),
        JsStrategy::Hybrid => {
            let l1 = WreqDownloader::new(timeout_secs, connect_timeout, tls_emulation)?;
            let l2 = ObscuraDownloader::new(timeout_secs);
            let l3 = ChromiumoxideDownloader::new(cookie_bridge);
            FetchRouter::Hybrid(Arc::new(HybridRouter::new(l1, l2, l3, ignore_waf)))
        },
        // Full renders every page directly in Chrome (no wreq → Obscura
        // escalation) and gates concurrency on system RAM via its own
        // ResourceGovernor to prevent OOM on large crawls.
        JsStrategy::Full => {
            let dl = ChromiumoxideDownloader::new(cookie_bridge);
            let governor = ResourceGovernor::new();
            FetchRouter::Full(Arc::new(dl), Arc::new(governor))
        },
    })
}

impl Downloader for FetchRouter {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        match self {
            Self::Static(dl) => dl.fetch(url),
            Self::Hybrid(dl) => dl.fetch(url),
            Self::Full(dl, governor) => Box::pin(async move {
                governor.check_resources().map_err(DownloadError::from)?;
                let _permit = governor.acquire().await?;
                dl.fetch(url).await
            }),
        }
    }

    fn supports_interactions(&self) -> bool {
        match self {
            Self::Static(dl) => dl.supports_interactions(),
            Self::Hybrid(dl) => dl.supports_interactions(),
            Self::Full(dl, _) => dl.supports_interactions(),
        }
    }

    fn memory_cost(&self) -> usize {
        match self {
            Self::Static(dl) => dl.memory_cost(),
            Self::Hybrid(dl) => dl.memory_cost(),
            Self::Full(dl, _) => dl.memory_cost(),
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
    /// Root correlation ID for this crawl — all pages share its `trace_id`.
    correlation_id: CorrelationId,
    collector: ResultsCollector,
    visited: Arc<UrlDeduplicator>,
    /// String URLs for checkpoint persistence (mirrors `visited` hashes).
    visited_urls: Arc<RwLock<Vec<String>>>,
    queue: Arc<UrlQueue>,
    rate_limiter: SharedRateLimiter,
    error_count: Arc<AtomicUsize>,
    /// Per-category error counters indexed by `CrawlErrorCategory::index()` (issue #374).
    error_breakdown: Arc<[AtomicUsize; 8]>,
    /// Checkpoint store for persistence (stateless — always available).
    checkpoint_store: BincodeCheckpoint,
    /// Loaded checkpoint state for crash recovery (`None` = fresh start).
    checkpoint_state: Option<CrawlCheckpoint>,
    /// Path to save checkpoint files.
    checkpoint_path: Option<PathBuf>,
    /// Pages between automatic checkpoint saves (0 = disabled).
    checkpoint_interval: u64,
    /// Skip robots.txt enforcement.
    ignore_robots: bool,
    /// Shared robots.txt fetcher for the crawl session (TLS-fingerprinted, #337).
    robots_fetcher: Arc<RobotsFetcher>,
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
        let error_breakdown = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let pages_crawled = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Robots.txt fetcher — shares the crawl's TLS fingerprint so the
        // robots.txt request is indistinguishable from a page fetch (#337).
        let robots_fetcher = Arc::new(
            RobotsFetcher::new(config_clone.tls_emulation, config_clone.timeout_secs)
                .map_err(|e| CrawlError::Internal(e.to_string()))?,
        );

        Ok(Self {
            config,
            correlation_id: CorrelationId::new(),
            collector,
            visited,
            visited_urls,
            queue,
            rate_limiter,
            error_count,
            error_breakdown,
            checkpoint_store: BincodeCheckpoint::new(),
            checkpoint_state: None,
            checkpoint_path: None,
            checkpoint_interval: 100,
            ignore_robots,
            robots_fetcher,
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

    /// Override the crawl's root correlation ID.
    ///
    /// Used by `crawl_site` / `crawl_site_with_options` to make the entry-point
    /// tracing span share the same `trace_id` as the engine and all its pages.
    fn with_correlation_id(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    /// Enable checkpoint persistence with the given interval and base directory.
    ///
    /// If the checkpoint directory cannot be created, checkpointing is disabled
    /// and an error is logged — the engine will NOT silently pretend to
    /// checkpoint while every save fails.
    pub fn with_checkpoint(mut self, interval: u64, base_dir: PathBuf) -> Self {
        let cp_path = CheckpointPath::new(&base_dir);
        if let Err(e) = cp_path.ensure_dir() {
            tracing::error!(
                error = %e,
                path = %base_dir.display(),
                "checkpoint dir creation failed — disabling checkpoint"
            );
            return self;
        }

        match self.checkpoint_store.load(&cp_path.file()) {
            Some(cp) => {
                info!(
                    "Resuming from checkpoint: {} visited, {} pages",
                    cp.visited.len(),
                    cp.pages_crawled
                );
                self.checkpoint_state = Some(cp);
            },
            None => {
                warn!("No checkpoint found, starting fresh");
                self.checkpoint_state = Some(CrawlCheckpoint::new());
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
    ///
    /// `tls_emulation` is the TLS/HTTP2 fingerprint profile applied to the wreq
    /// layer of the fetch router. `ignore_waf` bypasses WAF classification on
    /// the hybrid spa-detection path (REQ-WAF-07).
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Internal`] if the wreq client cannot be built.
    pub fn with_js_strategy(
        mut self,
        strategy: JsStrategy,
        tls_emulation: Profile,
        ignore_waf: bool,
    ) -> Result<Self, DownloadError> {
        let timeout = self.config.timeout_secs;
        let router = build_fetch_router(
            &strategy,
            timeout,
            tls_emulation,
            Arc::clone(&self.cookie_bridge),
            ignore_waf,
        )?;
        self.js_strategy = strategy;
        self.fetch_router = Some(router);
        Ok(self)
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

    #[allow(dead_code)] // pub(crate) for Phase 0 missing-docs triage — used in production builds
    /// Add an output stage that receives items after pipeline processing.
    pub(crate) fn add_output_stage(&mut self, stage: Box<dyn OutputStage>) {
        self.output_stages.push(Arc::from(stage));
    }

    /// Save the current checkpoint to disk (non-blocking wrapper).
    async fn save_checkpoint(&self) {
        if let Some(path) = &self.checkpoint_path {
            let visited_set: HashSet<String> = {
                #[allow(clippy::expect_used)]
                let urls = self
                    .visited_urls
                    .read()
                    .expect("visited_urls RwLock poisoned");
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
            let state = CrawlCheckpoint {
                visited: visited_set,
                queued: Vec::new(),
                pages_crawled: pages,
                banned_domains: banned,
                version: 1,
            };

            // Save on blocking thread to avoid blocking the event loop
            let store = BincodeCheckpoint::new();
            let path = path.clone();
            match tokio::task::spawn_blocking(move || store.save(&state, &path))
                .in_current_span()
                .await
            {
                Ok(Ok(())) => {
                    tracing::debug!("checkpoint saved successfully");
                },
                Ok(Err(e)) => {
                    tracing::error!(error = %e, "checkpoint save failed");
                },
                Err(join_err) => {
                    tracing::error!(error = %join_err, "checkpoint save task panicked");
                },
            }
        }
    }

    /// Spawn a signal handler that sets the shutdown flag on SIGINT/SIGTERM.
    fn spawn_signal_handler(shutdown: ShutdownSignal) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                #[allow(clippy::expect_used)]
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
        if let Some(ref cp) = self.checkpoint_state {
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
            correlation_id: self.correlation_id.clone(),
            visited: Arc::clone(&self.visited),
            visited_urls: Arc::clone(&self.visited_urls),
            queue: Arc::clone(&self.queue),
            rate_limiter: self.rate_limiter.clone(),
            session_pool: self.session_pool.clone(),
            ignore_robots: self.ignore_robots,
            robots_fetcher: Arc::clone(&self.robots_fetcher),
            error_count: Arc::clone(&self.error_count),
            error_breakdown: Arc::clone(&self.error_breakdown),
            pages_crawled: Arc::clone(&self.pages_crawled),
            collector: self.collector.clone(),
            cookie_bridge: Arc::clone(&self.cookie_bridge),
            banned_domains: Arc::clone(&self.banned_domains),
            fetch_router: self.fetch_router.clone(),
            pipeline: self.pipeline.clone(),
            output_stages: self.output_stages.to_vec(),
        });

        // Progress tracking start (issue #356 Fase 4)
        let start = std::time::Instant::now();

        // Main crawl loop
        while !url_queue.is_empty() || !tasks.is_empty() {
            // Check shutdown signal
            if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Shutdown signal received — saving checkpoint and exiting");
                self.save_checkpoint().await;
                break;
            }

            // Check if we've reached max pages (sin lock - atomic)
            if self.collector.is_full(config_clone.max_pages) {
                info!("Reached max pages limit: {}", config_clone.max_pages);
                break;
            }

            // Process completed tasks FIRST (non-blocking)
            while let Some(result) = tasks.try_join_next() {
                handle_crawl_result(result, &self.error_count, &self.error_breakdown);
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

                    // Periodic structured progress log (issue #356 Fase 4)
                    let progress =
                        CrawlProgress::new(pages, config_clone.max_pages, start.elapsed());
                    info!(
                        pages_crawled = pages,
                        max_pages = config_clone.max_pages,
                        progress_pct = progress.progress_pct(),
                        elapsed_secs = start.elapsed().as_secs(),
                        pages_per_sec = progress.pages_per_sec(),
                        eta_secs = progress.eta_secs(),
                        trace_id = %self.correlation_id.trace_id(),
                        "crawl progress"
                    );
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
                    handle_crawl_result(result, &self.error_count, &self.error_breakdown);
                }
            }
        }

        // Wait for remaining tasks
        while let Some(result) = tasks.join_next().await {
            handle_crawl_result(result, &self.error_count, &self.error_breakdown);
        }

        // Drop task_ctx so all cloned Senders inside CrawlTaskCtx are released.
        // Without this, collect() hangs forever — the mpsc channel stays open
        // because a Sender clone inside the Arc<CrawlTaskCtx> is still alive.
        drop(task_ctx);

        // Final checkpoint save
        self.save_checkpoint().await;

        // Collect results via mpsc channel — now all Senders are dropped,
        // so the receiver worker will drain and terminate.
        let collected_urls = std::mem::take(&mut self.collector).collect().await;
        let total_pages = collected_urls.len();
        let errors = self.error_count.load(std::sync::atomic::Ordering::SeqCst);

        let breakdown: std::collections::BTreeMap<CrawlErrorCategory, usize> =
            CrawlErrorCategory::ALL
                .iter()
                .filter_map(|cat| {
                    let count =
                        self.error_breakdown[cat.index()].load(std::sync::atomic::Ordering::SeqCst);
                    (count > 0).then_some((*cat, count))
                })
                .collect();

        // Structured crawl summary (issue #356 Fase 4, error breakdown #374)
        let duration = start.elapsed();
        let succeeded = total_pages.saturating_sub(errors);
        let summary_rate = if duration.as_secs_f64() > 0.0 {
            total_pages as f64 / duration.as_secs_f64()
        } else {
            0.0
        };
        info!(
            total_pages = total_pages,
            succeeded = succeeded,
            errors = errors,
            errors_waf = self.error_breakdown[CrawlErrorCategory::Waf.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_http = self.error_breakdown[CrawlErrorCategory::Http.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_timeout = self.error_breakdown[CrawlErrorCategory::Timeout.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_network = self.error_breakdown[CrawlErrorCategory::Network.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_rate_limit = self.error_breakdown[CrawlErrorCategory::RateLimit.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_extraction = self.error_breakdown[CrawlErrorCategory::Extraction.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_internal = self.error_breakdown[CrawlErrorCategory::Internal.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_panic = self.error_breakdown[CrawlErrorCategory::Panic.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            duration_secs = duration.as_secs(),
            pages_per_sec = summary_rate,
            trace_id = %self.correlation_id.trace_id(),
            "crawl completed"
        );

        Ok(CrawlResult::new(
            collected_urls,
            total_pages,
            errors,
            breakdown,
        ))
    }

    /// Graceful shutdown — drop the collector sender, receiver drains remaining items
    pub async fn shutdown(mut self) {
        // Abort signal handler to prevent the runtime from hanging
        if let Some(handle) = self.signal_handle.take() {
            handle.abort();
        }

        // Save checkpoint before shutting down
        self.save_checkpoint().await;

        info!("Engine shutdown complete");
    }
}

/// Handle result from a completed crawl task
fn handle_crawl_result(
    result: std::result::Result<Result<(), CrawlError>, tokio::task::JoinError>,
    error_count: &Arc<AtomicUsize>,
    error_breakdown: &Arc<[AtomicUsize; 8]>,
) {
    match result {
        Ok(Ok(())) => {
            // Task completed successfully
        },
        Ok(Err(e)) => {
            let category = CrawlErrorCategory::from(&e);
            warn!("Task error: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            error_breakdown[category.index()].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
        Err(e) => {
            warn!("Task panicked: {}", e);
            error_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            error_breakdown[CrawlErrorCategory::Panic.index()]
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

    // Per-page correlation (issue #356): share the crawl's trace_id, fresh
    // span_id. Lets a whole crawl be reconstructed by trace_id while each
    // page stays distinguishable by span_id.
    let page_correlation = ctx.correlation_id.child();
    let page_span = span!(
        Level::DEBUG,
        "crawl_page",
        correlation_id = %page_correlation,
        trace_id = %page_correlation.trace_id(),
        url = %url_str,
        depth = url_depth
    );
    let _page_guard = page_span.enter();

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
                log_scrape_error(
                    &msg,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "WAF challenge detected",
                );
                return Err(DownloadError::WafChallenge(msg).into());
            },
            Err(e) => {
                log_scrape_error(
                    &e,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "page fetch failed",
                );
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
                log_scrape_error(
                    &e,
                    &url_str,
                    "fetch",
                    Some(&page_correlation),
                    "page fetch failed",
                );
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
                                    || ctx.robots_fetcher.is_allowed(&link, link_domain).await)
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
                ctx.error_breakdown[CrawlErrorCategory::Extraction.index()]
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
#[derive(Debug, Clone)]
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
    /// TLS/HTTP2 fingerprint profile applied to the wreq fetch layer.
    ///
    /// Defaults to [`Profile::Chrome145`], the historical hardcoded fingerprint.
    pub tls_emulation: Profile,
    /// Bypass WAF classification on the hybrid spa-detection path (REQ-WAF-07).
    ///
    /// Defaults to `false`. When `true`, a genuine T1 challenge is treated per
    /// normal spa/static logic instead of aborting the fetch.
    pub ignore_waf: bool,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            checkpoint_path: None,
            session_pool_enabled: false,
            ignore_robots: false,
            js_strategy: JsStrategy::default(),
            autoscale_enabled: false,
            tls_emulation: Profile::Chrome145,
            ignore_waf: false,
        }
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
/// use webfang_core::{domain::CrawlerConfig, application::crawl_site};
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
    let correlation_id = CorrelationId::new();
    let span = span!(
        Level::INFO,
        "crawl_site",
        correlation_id = %correlation_id,
        trace_id = %correlation_id.trace_id(),
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
    let mut engine = Engine::new(config, ignore_robots)?.with_correlation_id(correlation_id);
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
/// use webfang_core::{domain::CrawlerConfig, application::crawl_site_with_options};
/// use webfang_core::application::crawler::engine::EngineOptions;
/// use webfang_core::domain::JsStrategy;
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
///     js_strategy: JsStrategy::Static,
///     autoscale_enabled: true,
///     ..Default::default()
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
    let correlation_id = CorrelationId::new();
    let span = span!(
        Level::INFO,
        "crawl_site_with_options",
        correlation_id = %correlation_id,
        trace_id = %correlation_id.trace_id(),
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

    let mut engine =
        Engine::new(config, options.ignore_robots)?.with_correlation_id(correlation_id);

    // Apply checkpoint if path provided
    if let Some(ref path) = options.checkpoint_path {
        engine = engine.with_checkpoint(100, path.clone());
    }

    // Apply session pool if enabled
    if options.session_pool_enabled {
        engine = engine.with_session_pool(Duration::from_secs(2));
    }

    // Apply JS strategy
    engine = engine.with_js_strategy(
        options.js_strategy,
        options.tls_emulation,
        options.ignore_waf,
    )?;

    // Apply autoscale if enabled
    if options.autoscale_enabled {
        engine = engine.with_autoscale();
    }

    let result = engine.run().await;
    engine.shutdown().await;
    result
}

#[cfg(test)]
#[cfg(not(miri))]
mod router_tests {
    use super::*;
    use crate::domain::JsStrategy;
    use std::sync::RwLock;

    fn test_cookie_bridge() -> Arc<RwLock<CookieBridge>> {
        Arc::new(RwLock::new(CookieBridge::new()))
    }

    #[test]
    fn build_fetch_router_static_returns_static_variant() {
        let router = build_fetch_router(
            &JsStrategy::Static,
            30,
            Profile::Chrome145,
            test_cookie_bridge(),
            false,
        )
        .expect("static router must build");
        assert!(
            matches!(router, FetchRouter::Static(_)),
            "Static strategy must produce FetchRouter::Static"
        );
    }

    #[test]
    fn build_fetch_router_hybrid_returns_hybrid_variant() {
        let router = build_fetch_router(
            &JsStrategy::Hybrid,
            30,
            Profile::Chrome145,
            test_cookie_bridge(),
            false,
        )
        .expect("hybrid router must build");
        assert!(
            matches!(router, FetchRouter::Hybrid(_)),
            "Hybrid strategy must produce FetchRouter::Hybrid"
        );
    }

    #[test]
    fn build_fetch_router_full_returns_full_variant() {
        let router = build_fetch_router(
            &JsStrategy::Full,
            30,
            Profile::Chrome145,
            test_cookie_bridge(),
            false,
        )
        .expect("full router must build");
        assert!(
            matches!(router, FetchRouter::Full(..)),
            "Full strategy must produce FetchRouter::Full (chrome-direct), not Hybrid"
        );
    }
}
