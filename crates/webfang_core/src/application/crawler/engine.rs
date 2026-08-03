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

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, span, warn, Instrument, Level};
use wreq_util::Profile;

use super::checkpoint::{
    BannedDomain, BincodeCheckpoint, CheckpointPath, CheckpointStore, CrawlCheckpoint,
};
use super::collector::ResultsCollector;
use super::concurrency_level::{ConcurrencyLevel, SharedConcurrencyLevel};
use super::crawl_scheduler::CrawlScheduler;
use super::crawl_task::{handle_crawl_result, run_crawl_task};
use super::fetch_router::{build_fetch_router, FetchRouter};
use super::ports;
use super::progress::CrawlProgress;
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::application::pipeline::{OutputStage, PipelineExecutor};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::clock::SystemClock;
use crate::domain::{
    CorrelationId, CrawlError, CrawlErrorCategory, CrawlResult, CrawlerConfig, JsStrategy,
};
use crate::infrastructure::crawler::robots_utils::RobotsFetcher;
use crate::infrastructure::downloader::cookie_bridge::CookieBridge;
use crate::infrastructure::downloader::resource_governor::ResourceGovernor;
use crate::infrastructure::downloader::DownloadError;
use crate::infrastructure::network::session_pool::{DomainSessionPool, SessionPoolConfig};

/// Shared shutdown signal — set to `true` when SIGINT/SIGTERM received.
type ShutdownSignal = Arc<AtomicBool>;

/// Grace added to the longest legitimate worker wait when bounding joins (#509).
///
/// The longest legitimate wait is a fetch (`config.timeout_secs`) or a
/// rate-limit token (`config.delay_ms`); anything beyond that plus this grace
/// is treated as a hung worker.
const SHUTDOWN_GRACE_SECS: u64 = 10;

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
    /// Scheduling policy — visited dedup, pending-work buffer, concurrency limits.
    scheduler: CrawlScheduler,
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
    /// Engine-wide cancellation token (#509) — fired on shutdown so workers
    /// blocked on rate-limit or resource-governor waits abort promptly.
    cancel_token: CancellationToken,
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

        // Scheduling policy owns the visited set, its checkpoint string mirror,
        // and the shared discovery queue (Arc-shared with the per-page tasks).
        let scheduler = CrawlScheduler::new(config_clone.concurrency);

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
            scheduler,
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
            cancel_token: CancellationToken::new(),
            js_strategy: JsStrategy::default(),
            fetch_router: None,
            cookie_bridge: Arc::new(RwLock::new(CookieBridge::new())),
            banned_domains: Arc::new(RwLock::new(Vec::new())),
            pipeline: None,
            output_stages: Vec::new(),
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
            // #503: the Engine path keeps today's rotating default behavior —
            // `CrawlerConfig.user_agent` is a separate dead field, out of scope.
            None,
            // #509: the Full strategy's governor shares the engine token so
            // permit waits abort on shutdown.
            self.cancel_token.clone(),
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

        self.scheduler.set_autoscale(level);
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
            let visited_set: HashSet<String> = self.scheduler.snapshot_visited();
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
    ///
    /// Also fires the cancellation token (#509) so workers blocked on
    /// rate-limit or resource-governor waits abort instead of hanging.
    fn spawn_signal_handler(
        shutdown: ShutdownSignal,
        cancel: CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
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
            cancel.cancel();
        })
    }

    /// Clone of the engine's cancellation token (#509).
    ///
    /// Fire it to abort workers blocked on rate-limit or resource-governor
    /// waits and unblock [`run`](Self::run)'s drain — the same effect as a
    /// signal-driven shutdown, without an OS signal.
    #[must_use]
    pub fn cancel_handle(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Run the crawl loop until completion
    ///
    /// Returns the collected URLs and error count.
    pub async fn run(&mut self) -> Result<CrawlResult, CrawlError> {
        let config_clone = Arc::clone(&self.config);

        // Spawn signal handler for graceful shutdown
        self.signal_handle = Some(Self::spawn_signal_handler(
            Arc::clone(&self.shutdown),
            self.cancel_token.clone(),
        ));

        // Load checkpoint state if resuming
        if let Some(ref cp) = self.checkpoint_state {
            if !cp.visited.is_empty() {
                self.scheduler.restore_visited(&cp.visited);
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

        // Seed the scheduler (pushes onto the discovery queue and pending buffer)
        self.scheduler.seed(&config_clone.seed_url).await;

        let mut tasks = tokio::task::JoinSet::new();

        // Build shared task context once — all spawned tasks share this Arc
        let task_ctx = Arc::new(CrawlTaskCtx {
            config: Arc::clone(&self.config),
            correlation_id: self.correlation_id.clone(),
            queue: self.scheduler.queue(),
            rate_limiter: self.rate_limiter.clone(),
            cancel_token: self.cancel_token.clone(),
            session_pool: self
                .session_pool
                .as_ref()
                .map(|p| Arc::new(p.clone()) as Arc<dyn crate::domain::session_port::SessionPort>),
            ignore_robots: self.ignore_robots,
            robots_checker: Arc::new(ports::ProductionRobotsChecker {
                fetcher: Arc::clone(&self.robots_fetcher),
            }),
            error_count: Arc::clone(&self.error_count),
            error_breakdown: Arc::clone(&self.error_breakdown),
            pages_crawled: Arc::clone(&self.pages_crawled),
            collector: Arc::new(ports::ProductionCollector {
                collector: self.collector.clone(),
            }),
            cookie_bridge: Arc::clone(&self.cookie_bridge),
            banned_domains: Arc::clone(&self.banned_domains),
            fetcher: Arc::new(ports::ProductionPageFetcher {
                router: self.fetch_router.clone(),
            }),
            link_extractor: Arc::new(ports::ProductionLinkExtractor),
            pipeline: self.pipeline.as_ref().map(|p| {
                Arc::new(ports::ProductionPipeline {
                    executor: Arc::clone(p),
                }) as Arc<dyn ports::ContentPipeline>
            }),
            output_stages: self.output_stages.to_vec(),
        });

        // Progress tracking start (issue #356 Fase 4)
        let start = std::time::Instant::now();

        // Bound every worker wait (#509): the longest legitimate wait is a
        // fetch (timeout_secs) or a rate-limit token (delay_ms), plus grace.
        let worker_wait_bound = Duration::from_secs(
            config_clone
                .timeout_secs
                .max(config_clone.delay_ms / 1000)
                .saturating_add(SHUTDOWN_GRACE_SECS),
        );

        // Main crawl loop
        while self.scheduler.has_pending_work() || !tasks.is_empty() {
            // Check shutdown signal
            if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Shutdown signal received — saving checkpoint and exiting");
                // Unblock workers parked on rate-limit/governor waits (#509)
                // before saving state; cancel() is idempotent.
                self.cancel_token.cancel();
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
            self.scheduler.drain_discovered().await;

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

            // Spawn new tasks up to the (autoscale-aware) concurrency limit.
            // The scheduler checks the limit before popping and skips URLs that
            // were already visited, marking each handed-out URL as visited.
            while let Some(discovered_url) = self.scheduler.next_url(tasks.len()) {
                // Spawn task — single Arc clone instead of 18 individual clones
                let task_ctx = Arc::clone(&task_ctx);
                tasks.spawn(
                    async move { run_crawl_task(task_ctx, discovered_url).await }.in_current_span(),
                );
            }

            // If no tasks can be spawned and work remains, wait for one task.
            // The wait is bounded (#509): on expiry we re-check engine state
            // instead of hanging on a wedged worker.
            if !self.scheduler.can_spawn(tasks.len()) && self.scheduler.has_pending_work() {
                match tokio::time::timeout(worker_wait_bound, tasks.join_next()).await {
                    Ok(Some(result)) => {
                        handle_crawl_result(result, &self.error_count, &self.error_breakdown);
                    },
                    Ok(None) => {},
                    Err(_elapsed) => {
                        warn!(
                            bound_secs = worker_wait_bound.as_secs(),
                            "worker wait exceeded bound — re-checking engine state"
                        );
                    },
                }
            }
        }

        // Wait for remaining tasks — bounded, then abort stragglers, so a
        // shutdown can never hang on a blocked worker (#509).
        Self::drain_tasks(
            &mut tasks,
            &self.error_count,
            &self.error_breakdown,
            worker_wait_bound,
        )
        .await;

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

    /// Drain spawned tasks with a bounded wait, aborting any stragglers so a
    /// shutdown can never hang on a blocked worker (#509).
    ///
    /// Graceful path: cancellation fires upstream, workers parked on
    /// rate-limit/governor waits return [`CrawlError::Cancelled`] promptly,
    /// and the drain finishes well within `bound`. `bound` only bites when a
    /// worker is genuinely wedged (e.g. a fetch ignoring its own timeout).
    async fn drain_tasks(
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        error_count: &Arc<AtomicUsize>,
        error_breakdown: &Arc<[AtomicUsize; 8]>,
        bound: Duration,
    ) {
        let drained = tokio::time::timeout(bound, async {
            while let Some(result) = tasks.join_next().await {
                handle_crawl_result(result, error_count, error_breakdown);
            }
        })
        .await
        .is_ok();

        if !drained {
            let remaining = tasks.len();
            warn!(
                remaining_tasks = remaining,
                bound_secs = bound.as_secs(),
                "task drain timed out — aborting remaining workers"
            );
            tasks.abort_all();
            while let Some(result) = tasks.join_next().await {
                handle_crawl_result(result, error_count, error_breakdown);
            }
        }
    }

    /// Graceful shutdown — drop the collector sender, receiver drains remaining items
    pub async fn shutdown(mut self) {
        // Unblock any worker still parked on a rate-limit/governor wait (#509).
        self.cancel_token.cancel();

        // Abort signal handler to prevent the runtime from hanging
        if let Some(handle) = self.signal_handle.take() {
            handle.abort();
        }

        // Save checkpoint before shutting down
        self.save_checkpoint().await;

        info!("Engine shutdown complete");
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
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
#[cfg(not(miri))] // wiremock + wreq use boring-sys2 FFI (unsupported by Miri)
mod tests {
    use super::*;
    use url::Url;
    use wiremock::matchers::path;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Shutdown while a worker waits: cancellation must abort the parked
    /// worker and `run()` must return within seconds, not after the 60s
    /// rate-limit refill (#509 acceptance).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_aborts_rate_blocked_worker_and_run_returns_within_bound() {
        let server = MockServer::start().await;
        let port = server.address().port();
        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><a href=\"http://127.0.0.1:{port}/linked\">next</a></body></html>"
            )))
            .mount(&server)
            .await;
        Mock::given(path("/linked"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>linked</body></html>"),
            )
            .mount(&server)
            .await;

        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
        let config = CrawlerConfig::builder(seed)
            .max_depth(1)
            .max_pages(10)
            .concurrency(1)
            .delay_ms(60_000) // 60s refill: without cancellation run() hangs ~1 min
            .timeout_secs(5)
            .ignore_robots(true)
            .build();

        let mut engine = Engine::new(config, true).expect("engine must build");
        let cancel = engine.cancel_handle();

        let run_handle = tokio::spawn(async move { engine.run().await });

        // Wait until the seed fetch lands; its discovered /linked task is then
        // dispatched into the rate-limit wait (burst already consumed). Even if
        // cancellation wins that race, the task still returns Cancelled without
        // fetching — both orderings must satisfy the assertions below.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let seed_received = server
                .received_requests()
                .await
                .map(|reqs| reqs.iter().any(|r| r.url.path() == "/"))
                .unwrap_or(false);
            if seed_received {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "seed request never arrived"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;

        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(10), run_handle)
            .await
            .expect("run() must return within the bound after cancellation")
            .expect("spawned run task must not panic");
        let crawl = result.expect("cancelled crawl must still return a result");

        assert_eq!(crawl.total_pages, 1, "only the seed was fetched");
        assert_eq!(crawl.errors, 0, "cancelled tasks are control signals");

        let requested_linked = server
            .received_requests()
            .await
            .map(|reqs| reqs.iter().any(|r| r.url.path() == "/linked"))
            .unwrap_or(true);
        assert!(
            !requested_linked,
            "rate-blocked worker must be cancelled before fetching"
        );
    }
}
