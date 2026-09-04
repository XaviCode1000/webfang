//! Engine module — Crawl orchestration with JoinSet-based concurrency
//!
//! The Engine manages the crawl loop, spawning tasks via JoinSet
//! with backpressure and rate limiting. Each task fetches a URL,
//! extracts links, and pushes discovered URLs to the queue.
//!
//! # D6 lock-across-await audit (task 2.3, change stabilization-concurrency-budget)
//!
//! Functions rewired by commit f5114cd6 (rate-limiter construction):
//!
//! | Function | `.await` points | Guard discipline | Verdict |
//! |---|---|---|---|
//! | `rate_limiter_config` | none (sync fn) | no lock guards touched | PASS |
//! | `Engine::with_budget` | none (sync ctor) | `SharedRateLimiter::new` / `CrawlScheduler::new` consume args by value; no `std`/`RwLock`/`Mutex`/`DashMap` guard outlives its expression | PASS |
//!
//! Enforcement: `#![deny(clippy::await_holding_lock)]` below fails the build if
//! a future edit ever holds a `std` lock guard across an `.await` in this module.

#![deny(clippy::await_holding_lock)]

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, instrument, warn, Instrument};
use wreq_util::Profile;

use super::checkpoint::{
    BannedDomain, BincodeCheckpoint, CheckpointPath, CheckpointStore, CrawlCheckpoint,
};
use super::collector::ResultsCollector;
use super::concurrency_level::{ConcurrencyLevel, SharedConcurrencyLevel};
use super::content_sink::CrawlContentSink;
use super::crawl_scheduler::CrawlScheduler;
use super::crawl_task::{handle_crawl_result, run_crawl_task};
use super::ports;
use super::progress::CrawlProgress;
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::application::pipeline::{OutputStage, PipelineExecutor};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::budget::{BudgetModel, BudgetOverrides};
use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::crawler_port::RobotsPort;
use crate::domain::downloader_factory::{
    DownloaderFactory, DownloaderSpec, DEFAULT_OBSCURA_BINARY,
};
use crate::domain::downloader_port::{DownloadError, Downloader};
use crate::domain::ram_probe_port::{system_default, RamProbePort};
use crate::domain::session_port::{SessionPoolConfig, SessionPort};
use crate::domain::{
    CorrelationId, CrawlError, CrawlErrorCategory, CrawlResult, CrawlerConfig, JsStrategy,
};

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
    /// Shared robots.txt port for the crawl session (TLS-fingerprinted, #337).
    /// The concrete `RobotsFetcher` is built by the composition-root helper
    /// `application::container::build_robots_fetcher` (ADR-0012-B post-narrow).
    robots_fetcher: Arc<dyn RobotsPort>,
    /// Optional domain session pool for per-domain rate limiting.
    ///
    /// Stored as the domain port `Arc<dyn SessionPort>` — the concrete
    /// `DomainSessionPool` is built by the composition-root helper
    /// `application::container::build_crawl_session_pool` (ADR-0012-B 3.F).
    session_pool: Option<Arc<dyn SessionPort>>,
    /// Atomic counter for total pages crawled (used by checkpoint and signal handler).
    pages_crawled: Arc<AtomicU64>,
    /// Shared shutdown signal for graceful termination.
    shutdown: ShutdownSignal,
    /// Engine-wide cancellation token (#509) — fired on shutdown so workers
    /// blocked on rate-limit or resource-governor waits abort promptly.
    cancel_token: CancellationToken,
    /// JavaScript rendering strategy.
    js_strategy: JsStrategy,
    /// Optional fetch downloader for hybrid/full JS rendering.
    ///
    /// Built through the injected [`DownloaderFactory`] — never constructed
    /// here. `None` means "no factory was injected", which is a fully
    /// supported state: [`ports::ProductionPageFetcher`] then falls back to
    /// the composition-root-injected [`StaticFetchPort`](crate::domain::crawler_port::StaticFetchPort),
    /// whose infrastructure implementation delegates to the static `fetch_url`
    /// free fn.
    fetch_router: Option<Arc<dyn Downloader>>,
    /// Factory that builds [`Self::fetch_router`]. `None` disables the
    /// dynamic fetch path for this engine.
    downloader_factory: Option<Arc<dyn DownloaderFactory>>,
    /// Cookie bridge for extracting and injecting cookies.
    cookie_bridge: Arc<RwLock<CookieBridge>>,
    /// Domains currently banned due to WAF or rate limiting.
    banned_domains: Arc<RwLock<Vec<BannedDomain>>>,
    /// Optional sink capturing every fetched page body (#631).
    content_sink: Option<Arc<dyn CrawlContentSink>>,
    /// Optional item pipeline for processing scraped content.
    pipeline: Option<Arc<PipelineExecutor>>,
    /// Output stages that receive items after pipeline processing.
    output_stages: Vec<Arc<Box<dyn OutputStage>>>,
    /// Optional handle for the signal handler task — aborted on shutdown
    /// to prevent the tokio runtime from hanging waiting for it.
    signal_handle: Option<tokio::task::JoinHandle<()>>,
    /// Immutable budget snapshot built once at entry; every derived tier
    /// (burst, crawl, domain) reads from it.
    budget: BudgetModel,
    /// System RAM-usage probe — domain port read by the autoscale loop
    /// (`with_autoscale`) to throttle crawl permits under memory pressure.
    /// Defaults to the sysinfo-backed production impl; tests inject a fake.
    /// ADR-0012 sub-slice 3.B-1c.
    ram_probe: Arc<dyn RamProbePort>,
}

/// Rate-limiter burst source derived from the budget model (design D4/D1):
/// the burst is an INDEPENDENT tier — it must never re-read
/// `CrawlerConfig.concurrency`, so raising the crawler concurrency leaves
/// the token-bucket burst unchanged.
fn rate_limiter_config(config: &CrawlerConfig, budget: &BudgetModel) -> RateLimiterConfig {
    RateLimiterConfig::new(config.delay_ms, budget.burst().get())
}

impl Engine {
    /// Create a new Engine from a CrawlerConfig
    ///
    /// Bug R2-1: the operator-level [`BudgetOverrides`] carried on the config
    /// feed `BudgetModel::build`, so an explicit `--concurrency` /
    /// `--rate-limit-burst` reaches the scheduler spawn bound and rate-limiter
    /// burst instead of being silently replaced by the auto table.
    pub(crate) fn new(config: CrawlerConfig, ignore_robots: bool) -> Result<Self, CrawlError> {
        let overrides = config.budget_overrides;
        Self::with_budget(config, ignore_robots, overrides)
    }

    /// Create an Engine whose concurrency budgets derive from a
    /// [`BudgetModel`] built ONCE at entry (design D4). `Engine::new`
    /// forwards the overrides carried on `CrawlerConfig`; direct callers of
    /// `with_budget` pass them explicitly (`BudgetModel::build` stays here so
    /// the model is constructed exactly once per crawl).
    fn with_budget(
        config: CrawlerConfig,
        ignore_robots: bool,
        overrides: BudgetOverrides,
    ) -> Result<Self, CrawlError> {
        let config = Arc::new(config);
        let config_clone = Arc::clone(&config);

        // ONE budget derivation per run — every tier below reads from this
        // immutable snapshot.
        let budget =
            BudgetModel::build(overrides, &crate::domain::budget::detector::SystemDetector);

        // Create rate limiter using SharedRateLimiter (single source of truth).
        // Burst derives from the model (Q1 DECOUPLE); delay_ms unchanged.
        let rate_limiter_config = rate_limiter_config(&config_clone, &budget);
        let rate_limiter = match SharedRateLimiter::new(&rate_limiter_config) {
            Ok(limiter) => limiter,
            // LCOV_EXCL_LINE defensive: rate-limiter-config — SharedRateLimiter::new fails only on invalid config, an invariant
            Err(e) => return Err(CrawlError::Internal(e.to_string())),
        };

        // Scheduling policy owns the visited set, its checkpoint string mirror,
        // and the shared discovery queue (Arc-shared with the per-page tasks).
        // The spawn bound derives from the model's Operation.crawl tier, not
        // from `CrawlerConfig.concurrency` (task 2.2b).
        let scheduler = CrawlScheduler::new(budget.crawl());

        // Results collector via mpsc channel
        let collector = ResultsCollector::new(config_clone.max_pages, Some(config_clone.max_pages));
        let error_count = Arc::new(AtomicUsize::new(0));
        let error_breakdown = Arc::new(std::array::from_fn(|_| AtomicUsize::new(0)));
        let pages_crawled = Arc::new(AtomicU64::new(0));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Robots.txt port — shares the crawl's TLS fingerprint so the
        // robots.txt request is indistinguishable from a page fetch (#337).
        // Built through the composition-root helper (trait in domain,
        // concrete in infra — ADR-0012-B post-narrow robots slice).
        let robots_fetcher = crate::application::container::build_robots_fetcher(
            config_clone.tls_emulation,
            config_clone.timeout_secs,
        )
        // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
        .map_err(|e| CrawlError::Internal(e.to_string()))?;

        Ok(Self {
            config,
            correlation_id: CorrelationId::new(),
            collector,
            scheduler,
            rate_limiter,
            budget,
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
            downloader_factory: None,
            cookie_bridge: Arc::new(RwLock::new(CookieBridge::new())),
            banned_domains: Arc::new(RwLock::new(Vec::new())),
            content_sink: None,
            pipeline: None,
            output_stages: Vec::new(),
            signal_handle: None,
            // Default to the sysinfo-backed probe so the autoscale loop is
            // wired without any extra setup. Tests inject a fake via
            // `Engine::with_ram_probe` (no real sysinfo reads in unit tests).
            // The factory is domain-side: `application` must not name the
            // infrastructure concrete (ADR-0012-B cheap win).
            ram_probe: system_default(),
        })
    }

    /// Override the crawl's root correlation ID.
    ///
    /// Used by `crawl_site` / `crawl_site_with_options` to make the entry-point
    /// tracing span share the same `trace_id` as the engine and all its pages.
    pub(crate) fn with_correlation_id(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    /// Unified persistence — wraps `with_checkpoint` when `PersistenceMode` enables checkpointing.
    ///
    /// `Checkpoint` and `Full` variants configure periodic checkpointing via
    /// `with_checkpoint`; `Disabled` and `Resume` leave checkpoint disabled.
    /// On IO error creating the checkpoint directory, `with_checkpoint` logs
    /// `error!` and disables checkpoint without failing the crawl (CRC32
    /// atomic guarantees preserved).
    pub fn with_persistence(self, mode: crate::domain::persistence::PersistenceMode) -> Self {
        if let Some(cfg) = mode.checkpoint_cfg() {
            self.with_checkpoint(cfg.interval, cfg.dir.clone())
        } else {
            self
        }
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
    ///
    /// Pure injection — the engine never constructs the pool. The concrete
    /// `DomainSessionPool` is built by the composition-root helper
    /// `application::container::build_crawl_session_pool` and handed over as
    /// the domain port, mirroring the `Engine::with_ram_probe()` precedent
    /// (ADR-0012-B sub-slice 3.B-1c).
    pub fn with_session_pool(mut self, pool: Arc<dyn SessionPort>) -> Self {
        self.session_pool = Some(pool);
        self
    }

    /// Inject the factory that builds the fetch downloader.
    ///
    /// Must be called before [`Self::with_js_strategy`], which is where the
    /// factory is actually invoked. Without it `with_js_strategy` records the
    /// strategy but leaves the downloader unset, and the crawl falls back to
    /// the static `fetch_url()` path — hybrid/full rendering silently
    /// degrades to static. Callers that need JS rendering own this wiring:
    /// `application` cannot name a concrete factory without re-introducing
    /// the `application → infrastructure` edge this port removes, so the
    /// composition happens in the gate-exempt `cli` layer.
    #[must_use]
    pub fn with_downloader_factory(mut self, factory: Arc<dyn DownloaderFactory>) -> Self {
        self.downloader_factory = Some(factory);
        self
    }

    /// Set the JavaScript rendering strategy.
    ///
    /// `tls_emulation` is the TLS/HTTP2 fingerprint profile applied to the wreq
    /// layer of the fetch router. `ignore_waf` bypasses WAF classification on
    /// the hybrid spa-detection path (REQ-WAF-07). `obscura_binary` is the
    /// Hybrid Layer 2 binary — a path is invoked as given, a bare name is
    /// resolved from `PATH` (#787).
    ///
    /// The downloader itself is built by the factory injected through
    /// [`Self::with_downloader_factory`]. When no factory was injected the
    /// strategy is still recorded but no downloader is built: there is no
    /// built-in fallback, because building one would mean `application`
    /// constructing infrastructure concretes again.
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Internal`] if the wreq client cannot be built.
    // 8 params: the strategy's full dependency set (profile, WAF, retry
    // backoff, obscura binary). Bundling them would only move the same
    // wiring one level up (same pattern as DownloaderSpec).
    #[allow(clippy::too_many_arguments)]
    pub fn with_js_strategy(
        mut self,
        strategy: JsStrategy,
        tls_emulation: Profile,
        ignore_waf: bool,
        max_retries: u32,
        backoff_base_ms: u64,
        backoff_max_ms: u64,
        obscura_binary: String,
    ) -> Result<Self, DownloadError> {
        let timeout = self.config.timeout_secs;
        // No factory injected => no downloader built. `None` is a supported
        // state: ProductionPageFetcher falls back to the static fetch_url().
        let router = self
            .downloader_factory
            .as_ref()
            .map(|factory| {
                factory.build(
                    &DownloaderSpec {
                        strategy,
                        timeout_secs: timeout,
                        tls_emulation,
                        ignore_waf,
                        // #503: the Engine path keeps today's rotating default
                        // behavior — `CrawlerConfig.user_agent` is a separate
                        // dead field, out of scope.
                        user_agent: None,
                        // #890: operator headers/cookies are wired on the scrape
                        // path (cli/scrape_flow.rs). The Engine path keeps
                        // profile-default behavior — same out-of-scope precedent
                        // as the UA above.
                        custom_headers: Vec::new(),
                        accept_language: None,
                        initial_cookie_jar: None,
                        max_retries,
                        backoff_base_ms,
                        backoff_max_ms,
                        obscura_binary,
                    },
                    // #509: the Full strategy's governor shares the engine token
                    // so permit waits abort on shutdown.
                    Arc::clone(&self.cookie_bridge),
                    self.cancel_token.clone(),
                )
            })
            .transpose()?;
        self.js_strategy = strategy;
        self.fetch_router = router;
        Ok(self)
    }

    /// Enable autoscaled concurrency based on system RAM.
    ///
    /// Spawns a background task that polls the injected [`RamProbePort`]
    /// every 5 seconds and adjusts the shared concurrency level accordingly.
    /// The engine's spawn loop reads this level to compute effective concurrency.
    pub fn with_autoscale(mut self) -> Self {
        let level = Arc::new(SharedConcurrencyLevel::new());
        let level_clone = Arc::clone(&level);
        let probe = Arc::clone(&self.ram_probe);

        tokio::spawn(
            async move {
                let mut interval = tokio::time::interval(Duration::from_secs(5));
                interval.tick().await; // skip first immediate tick
                loop {
                    interval.tick().await;
                    let usage = probe.ram_usage_percent().as_percent();
                    let new_level = if usage >= f32::from(crate::domain::budget::derivation::RamThresholds::DEFAULT_CRITICAL_PERCENT) {
                        ConcurrencyLevel::Critical
                    } else if usage >= f32::from(crate::domain::budget::derivation::RamThresholds::DEFAULT_WARNING_PERCENT) {
                        ConcurrencyLevel::Reduced
                    } else {
                        ConcurrencyLevel::Normal
                    };
                    if level_clone.get() != new_level {
                        info!(
                            "Autoscale: RAM {usage:.2}% → concurrency level {:?}",
                            new_level
                        );
                        level_clone.set(new_level);
                    }
                }
            }
            .in_current_span(),
        );

        self.scheduler.set_autoscale(level);
        self
    }

    /// Override the RAM-usage probe used by [`Self::with_autoscale`].
    ///
    /// Tests inject a deterministic fake so the autoscale loop's threshold
    /// branches can be exercised without real sysinfo reads. Production
    /// code can leave the default
    /// ([`SystemRamProbe`](crate::domain::ram_probe_port::SystemRamProbe), built
    /// by [`system_default`]) in
    /// place.
    #[must_use]
    pub fn with_ram_probe(mut self, probe: Arc<dyn RamProbePort>) -> Self {
        self.ram_probe = probe;
        self
    }

    /// Restore banned domains from a checkpoint.
    pub fn with_banned_domains(self, domains: Vec<BannedDomain>) -> Self {
        if let Ok(mut banned) = self.banned_domains.write() {
            *banned = domains;
        }
        self
    }

    /// Capture every fetched page body into `sink` (#631).
    ///
    /// [`CrawlResult`] is metadata only; without a sink the crawl discards the
    /// bodies after link extraction and callers (batch mode) have nothing to
    /// export. The sink is shared, so several engines can feed one collection.
    #[must_use]
    pub fn with_content_sink(mut self, sink: Arc<dyn CrawlContentSink>) -> Self {
        self.content_sink = Some(sink);
        self
    }

    /// Set the item pipeline executor for processing scraped content.
    pub fn with_pipeline(mut self, executor: PipelineExecutor) -> Self {
        self.pipeline = Some(Arc::new(executor));
        self
    }

    /// Save the current checkpoint to disk (non-blocking wrapper).
    async fn save_checkpoint(&self) {
        if let Some(path) = &self.checkpoint_path {
            let state = self.build_checkpoint_state().await;
            self.persist_checkpoint(state, path).await;
        }
    }

    /// Snapshot the current engine state into a checkpoint.
    async fn build_checkpoint_state(&self) -> CrawlCheckpoint {
        let visited_set: HashSet<String> = self.scheduler.snapshot_visited();
        let pages = self
            .pages_crawled
            .load(std::sync::atomic::Ordering::Relaxed);
        let banned = self
            .banned_domains
            .read()
            .map(|d| d.clone())
            .unwrap_or_default();
        CrawlCheckpoint {
            visited: visited_set,
            queued: self.scheduler.snapshot_pending().await,
            pages_crawled: pages,
            banned_domains: banned,
            version: 1,
        }
    }

    /// Persist a checkpoint on a blocking thread, logging the outcome.
    async fn persist_checkpoint(&self, state: CrawlCheckpoint, path: &std::path::Path) {
        // Save on blocking thread to avoid blocking the event loop
        let store = BincodeCheckpoint::new();
        let path = path.to_path_buf();
        let outcome = tokio::task::spawn_blocking(move || store.save(&state, &path))
            .in_current_span()
            .await;
        Self::log_checkpoint_save(outcome);
    }

    /// Log the result of a checkpoint save attempt.
    fn log_checkpoint_save(outcome: Result<Result<(), String>, tokio::task::JoinError>) {
        match outcome {
            Ok(Ok(())) => {
                tracing::debug!("checkpoint saved successfully");
            },
            Ok(Err(e)) => {
                tracing::error!(error = %e, "checkpoint save failed");
            },
            // LCOV_EXCL_START defensive: checkpoint-join-error — a JoinError occurs only when the spawned task panicked, a bug
            Err(join_err) => {
                tracing::error!(error = %join_err, "checkpoint save task panicked");
            },
            // LCOV_EXCL_STOP
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
        tokio::spawn(
            async move {
                let ctrl_c = tokio::signal::ctrl_c();
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    // SIGTERM registration failure is only possible when the OS
                    // rejects the handler (e.g. invalid stream or OS). Never panic —
                    // gracefully degrade to SIGINT-only (warn for observability).
                    match signal(SignalKind::terminate()) {
                        Ok(mut sigterm) => {
                            tokio::select! {
                                _ = ctrl_c => {
                                    info!("Received SIGINT — initiating graceful shutdown");
                                },
                                _ = sigterm.recv() => {
                                    info!("Received SIGTERM — initiating graceful shutdown");
                                },
                            }
                        },
                        // LCOV_EXCL_START defensive: signal-registration — the OS rejects the SIGTERM handler only on an invariant break
                        Err(e) => {
                            warn!(
                                error = %e,
                                "SIGTERM handler registration failed — graceful shutdown will only respond to SIGINT"
                            );
                            ctrl_c.await.ok();
                        },
                        // LCOV_EXCL_STOP
                    }
                }
                #[cfg(not(unix))]
                {
                    ctrl_c.await.ok();
                    info!("Received interrupt — initiating graceful shutdown");
                }
                shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            }
            .in_current_span(),
        )
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
        self.restore_checkpoint_state();

        // Seed the scheduler (pushes onto the discovery queue and pending buffer)
        self.scheduler.seed(&config_clone.seed_url).await;

        // Apply pattern filtering to seed — #634
        let seed_str = config_clone.seed_url.as_str().to_string();
        if !crate::application::url_filter::is_allowed(&seed_str, &config_clone) {
            info!(
                seed_url = %config_clone.seed_url,
                "Seed URL excluded by pattern filters — exiting with empty result"
            );
            return Ok(CrawlResult::empty());
        }

        let mut tasks = tokio::task::JoinSet::new();

        // Build shared task context once — all spawned tasks share this Arc
        let task_ctx = self.build_task_ctx();

        // Progress tracking start (issue #356 Fase 4)
        let start = std::time::Instant::now();

        // Bound every worker wait (#509): the longest legitimate wait is a
        // fetch (timeout_secs) or a rate-limit token (delay_ms), plus grace.
        let worker_wait_bound = Self::worker_wait_bound(&config_clone);

        // Main crawl loop
        self.crawl_loop(&mut tasks, &task_ctx, start, worker_wait_bound)
            .await;

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

        let breakdown = self.collect_error_breakdown();

        // Structured crawl summary (issue #356 Fase 4, error breakdown #374)
        self.log_crawl_summary(total_pages, errors, start);

        Ok(CrawlResult::new(
            collected_urls,
            total_pages,
            errors,
            breakdown,
        ))
    }

    /// Load checkpoint state if resuming (visited, queued, banned domains).
    fn restore_checkpoint_state(&mut self) {
        if let Some(cp) = &self.checkpoint_state {
            Self::restore_visited(&mut self.scheduler, &cp.visited);
            Self::restore_queued(&mut self.scheduler, &cp.queued);
            Self::restore_banned(&self.banned_domains, &cp.banned_domains);
        }
    }

    /// Restore the visited set from a checkpoint.
    fn restore_visited(scheduler: &mut CrawlScheduler, visited: &HashSet<String>) {
        if !visited.is_empty() {
            scheduler.restore_visited(visited);
            info!("Restored {} visited URLs from checkpoint", visited.len());
        }
    }

    /// Restore the pending queue from a checkpoint.
    fn restore_queued(scheduler: &mut CrawlScheduler, queued: &[String]) {
        if !queued.is_empty() {
            scheduler.restore_pending(queued);
            info!("Restored {} queued URLs from checkpoint", queued.len());
        }
    }

    /// Restore banned domains from a checkpoint.
    fn restore_banned(banned_domains: &RwLock<Vec<BannedDomain>>, cp_banned: &[BannedDomain]) {
        if !cp_banned.is_empty() {
            if let Ok(mut banned) = banned_domains.write() {
                *banned = cp_banned.to_vec();
            }
            info!(
                "Restored {} banned domains from checkpoint",
                cp_banned.len()
            );
        }
    }

    /// Build the shared task context once — all spawned tasks share this Arc.
    fn build_task_ctx(&self) -> Arc<CrawlTaskCtx> {
        Arc::new(CrawlTaskCtx {
            config: Arc::clone(&self.config),
            correlation_id: self.correlation_id.clone(),
            queue: self.scheduler.queue(),
            rate_limiter: self.rate_limiter.clone(),
            cancel_token: self.cancel_token.clone(),
            session_pool: self.session_pool.clone(),
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
                fallback: crate::application::container::build_static_fetcher(),
            }),
            link_extractor: Arc::new(ports::ProductionLinkExtractor::new(
                crate::application::container::build_link_extractor(),
            )),
            content_sink: self.content_sink.clone(),
            pipeline: self.pipeline.as_ref().map(|p| {
                Arc::new(ports::ProductionPipeline {
                    executor: Arc::clone(p),
                }) as Arc<dyn ports::ContentPipeline>
            }),
            output_stages: self.output_stages.to_vec(),
        })
    }

    /// Bound every worker wait (#509): the longest legitimate wait is a fetch
    /// (timeout_secs) or a rate-limit token (delay_ms), plus grace.
    fn worker_wait_bound(config: &CrawlerConfig) -> Duration {
        Duration::from_secs(
            config
                .timeout_secs
                .max(config.delay_ms / 1000)
                .saturating_add(SHUTDOWN_GRACE_SECS),
        )
    }

    /// Main crawl loop — process completed tasks, drain discovered links,
    /// checkpoint periodically, spawn new tasks, and wait for work.
    async fn crawl_loop(
        &mut self,
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        task_ctx: &Arc<CrawlTaskCtx>,
        start: std::time::Instant,
        worker_wait_bound: Duration,
    ) {
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
            if self.collector.is_full(self.config.max_pages) {
                info!("Reached max pages limit: {}", self.config.max_pages);
                break;
            }

            // Process completed tasks FIRST (non-blocking)
            self.process_completed_tasks(tasks);

            // Re-check max_pages AFTER processing — concurrent completions may
            // have pushed the collector past the limit while we were spawning.
            // Without this, spawn_available_tasks() below would keep fanning out
            // new workers beyond max_pages (issue #590, bug #1).
            if self.collector.is_full(self.config.max_pages) {
                info!(
                    "Reached max pages limit after processing: {}",
                    self.config.max_pages
                );
                self.cancel_token.cancel();
                break;
            }

            // Drain discovered links from the deduplicated UrlQueue
            self.scheduler.drain_discovered().await;

            // Periodic checkpoint save
            self.maybe_save_periodic_checkpoint(start).await;

            // Spawn new tasks up to the (autoscale-aware) concurrency limit.
            Self::spawn_available_tasks(&mut self.scheduler, tasks, task_ctx);

            // If no tasks can be spawned and work remains, wait for one task.
            self.wait_for_task(tasks, worker_wait_bound).await;
        }
    }

    /// Process completed tasks (non-blocking) and update error counters.
    fn process_completed_tasks(&self, tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>) {
        while let Some(result) = tasks.try_join_next() {
            handle_crawl_result(result, &self.error_count, &self.error_breakdown);
        }
    }

    /// Periodically save a checkpoint and emit a structured progress log.
    async fn maybe_save_periodic_checkpoint(&self, start: std::time::Instant) {
        if self.checkpoint_interval == 0 {
            return;
        }
        let pages = self
            .pages_crawled
            .load(std::sync::atomic::Ordering::Relaxed);
        if pages == 0 || !pages.is_multiple_of(self.checkpoint_interval) {
            return;
        }
        debug!("Periodic checkpoint save at {pages} pages");
        self.save_checkpoint().await;

        // Periodic structured progress log (issue #356 Fase 4)
        let progress = CrawlProgress::new(pages, self.config.max_pages, start.elapsed());
        info!(
            pages_crawled = pages,
            max_pages = self.config.max_pages,
            progress_pct = progress.progress_pct(),
            elapsed_secs = start.elapsed().as_secs(),
            pages_per_sec = progress.pages_per_sec(),
            eta_secs = progress.eta_secs(),
            trace_id = %self.correlation_id.trace_id(),
            "crawl progress"
        );
    }

    /// Spawn new tasks up to the (autoscale-aware) concurrency limit.
    ///
    /// The scheduler checks the limit before popping and skips URLs that
    /// were already visited, marking each handed-out URL as visited.
    fn spawn_available_tasks(
        scheduler: &mut CrawlScheduler,
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        task_ctx: &Arc<CrawlTaskCtx>,
    ) {
        while let Some(discovered_url) = scheduler.next_url(tasks.len()) {
            // Spawn task — single Arc clone instead of 18 individual clones
            let task_ctx = Arc::clone(task_ctx);
            tasks.spawn(
                async move { run_crawl_task(task_ctx, discovered_url).await }.in_current_span(),
            );
        }
    }

    /// Wait for one task when no more can be spawned, bounded so a wedged
    /// worker cannot hang the loop (#509).
    async fn wait_for_task(
        &self,
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        worker_wait_bound: Duration,
    ) {
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

    /// Build the per-category error breakdown map (issue #374).
    fn collect_error_breakdown(&self) -> std::collections::BTreeMap<CrawlErrorCategory, usize> {
        CrawlErrorCategory::ALL
            .iter()
            .filter_map(|cat| {
                let count =
                    self.error_breakdown[cat.index()].load(std::sync::atomic::Ordering::SeqCst);
                (count > 0).then_some((*cat, count))
            })
            .collect()
    }

    /// Emit the structured crawl summary (issue #356 Fase 4, error breakdown #374).
    fn log_crawl_summary(&self, total_pages: usize, errors: usize, start: std::time::Instant) {
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
    /// Pages between checkpoint saves (0 = disabled, but `checkpoint_path` None already disables).
    /// Defaults to 100 for backward compat; `PersistenceMode` overrides via `checkpoint_interval`.
    pub checkpoint_interval: u64,
    /// Enable the domain session pool for per-domain rate limiting.
    pub session_pool_enabled: bool,
    /// Skip robots.txt enforcement.
    pub ignore_robots: bool,
    /// JavaScript rendering strategy.
    pub js_strategy: JsStrategy,
    /// Obscura binary name or path for the Hybrid strategy's Layer 2 (#787).
    ///
    /// A path is invoked as given; a bare name is resolved from `PATH`.
    /// Defaults to `obscura`.
    pub obscura_binary: String,
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
    /// Maximum number of retry attempts for failed fetches.
    pub max_retries: u32,
    /// Base delay for exponential backoff (ms).
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff (ms).
    pub backoff_max_ms: u64,
    /// Factory that builds the fetch downloader for `js_strategy`.
    ///
    /// `None` (the default) means the engine records the strategy but builds
    /// no downloader, so the crawl uses the static `fetch_url()` path. Any
    /// caller that needs `Hybrid` or `Full` rendering MUST inject a factory —
    /// the only production implementation lives in
    /// [`crate::infrastructure::downloader::fetch_router::DefaultDownloaderFactory`],
    /// so wiring it is a `cli`/composition-root responsibility (`cli` is
    /// exempt from the ADR-0010 direction gate).
    pub downloader_factory: Option<Arc<dyn DownloaderFactory>>,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            checkpoint_path: None,
            checkpoint_interval: 100,
            session_pool_enabled: false,
            ignore_robots: false,
            js_strategy: JsStrategy::default(),
            // #787: keep today's `obscura`-on-PATH behavior for callers that
            // do not configure the binary.
            obscura_binary: DEFAULT_OBSCURA_BINARY.to_string(),
            autoscale_enabled: false,
            tls_emulation: Profile::Chrome145,
            ignore_waf: false,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            downloader_factory: None,
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
pub async fn crawl_site(config: CrawlerConfig) -> Result<CrawlResult, CrawlError> {
    crawl_site_inner(config, CorrelationId::new(), None).await
}

/// Crawl a website and capture every fetched page body into `sink`.
///
/// Same crawl semantics as [`crawl_site`], but the raw body of each fetched
/// page is handed to the sink before link extraction discards it.
/// [`CrawlResult`] carries metadata only, so this is the only way for a caller
/// to obtain crawl content without a second HTTP round-trip — the gap that
/// made `--batch` write zero files (#631).
///
/// # Errors
///
/// Returns [`CrawlError`] when the engine cannot be constructed or the crawl
/// loop fails, exactly as [`crawl_site`] does.
pub async fn crawl_site_capturing(
    config: CrawlerConfig,
    sink: Arc<dyn CrawlContentSink>,
) -> Result<CrawlResult, CrawlError> {
    crawl_site_inner(config, CorrelationId::new(), Some(sink)).await
}

/// Inner implementation of [`crawl_site`].
///
/// The `#[instrument]` span declares the run-root identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501): FileTraceLayer snapshots span fields
/// in `on_new_span`, so fields recorded later never reach the `--trace-file`
/// JSONL. The instrumented span lifecycle is also async-safe — no `enter()`
/// guard crosses an `.await` (#519).
#[instrument(
    name = "crawl_site",
    skip(config, correlation_id, content_sink),
    fields(
        correlation_id = %correlation_id,
        trace_id = %correlation_id.trace_id(),
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages,
        delay_ms = config.delay_ms,
        concurrency = config.concurrency
    )
)]
async fn crawl_site_inner(
    config: CrawlerConfig,
    correlation_id: CorrelationId,
    content_sink: Option<Arc<dyn CrawlContentSink>>,
) -> Result<CrawlResult, CrawlError> {
    info!(
        "Starting crawl from {} with max_depth={} max_pages={}",
        config.seed_url, config.max_depth, config.max_pages
    );

    let ignore_robots = config.ignore_robots;
    let mut engine = Engine::new(config, ignore_robots)?.with_correlation_id(correlation_id);
    if let Some(sink) = content_sink {
        engine = engine.with_content_sink(sink);
    }
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
pub async fn crawl_site_with_options(
    config: CrawlerConfig,
    options: EngineOptions,
) -> Result<CrawlResult, CrawlError> {
    crawl_site_with_options_inner(config, options, CorrelationId::new()).await
}

/// Inner implementation of [`crawl_site_with_options`].
///
/// The `#[instrument]` span declares the run-root identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501): FileTraceLayer snapshots span fields
/// in `on_new_span`. The instrumented span lifecycle is also async-safe — no
/// `enter()` guard crosses an `.await` (#519).
#[instrument(
    name = "crawl_site_with_options",
    skip(config, options, correlation_id),
    fields(
        correlation_id = %correlation_id,
        trace_id = %correlation_id.trace_id(),
        seed_url = %config.seed_url,
        max_depth = config.max_depth,
        max_pages = config.max_pages,
        checkpoint_enabled = options.checkpoint_path.is_some(),
        session_pool = options.session_pool_enabled,
        ignore_robots = options.ignore_robots
    )
)]
async fn crawl_site_with_options_inner(
    config: CrawlerConfig,
    options: EngineOptions,
    correlation_id: CorrelationId,
) -> Result<CrawlResult, CrawlError> {
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

    // Apply checkpoint if path provided — interval from options (PersistenceMode), not hardcoded.
    if let Some(ref path) = options.checkpoint_path {
        engine = engine.with_checkpoint(options.checkpoint_interval, path.clone());
    }

    // Apply session pool if enabled. ADR-0012-B 3.F: the engine consumes the
    // domain port — the concrete pool is built by the composition-root helper
    // in `application::container`. Slot count derives from the model's Domain
    // tier, not a raw default (task 2.2c); the 2s cooldown is the backoff
    // base delay, exactly as the former in-engine construction used.
    if options.session_pool_enabled {
        let pool_cfg = SessionPoolConfig {
            base_delay: Duration::from_secs(2),
            pool_size: engine.budget.domain(),
            ..SessionPoolConfig::default()
        };
        engine = engine.with_session_pool(crate::application::container::build_crawl_session_pool(
            pool_cfg,
        ));
    }

    // Apply the downloader factory before the JS strategy: `with_js_strategy`
    // is where the factory is invoked, and it has no built-in fallback.
    if let Some(factory) = options.downloader_factory.clone() {
        engine = engine.with_downloader_factory(factory);
    }

    // Apply JS strategy
    engine = engine.with_js_strategy(
        options.js_strategy,
        options.tls_emulation,
        options.ignore_waf,
        options.max_retries,
        options.backoff_base_ms,
        options.backoff_max_ms,
        // #787: propagate --obscura-binary into the Hybrid Layer 2 downloader.
        options.obscura_binary.clone(),
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
    use crate::domain::budget::detector::FixedDetector;
    use crate::domain::budget::tiers::{BurstPermits, CrawlConcurrency};
    use crate::domain::budget::{BudgetModel, BudgetOverrides};
    use url::Url;
    use wiremock::matchers::{path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Spec scenario (Q1 DECOUPLE): raising the configured crawler
    /// concurrency must NOT move the rate-limiter burst — the burst is a
    /// budget-model tier derived from the detector seam, independent of
    /// `CrawlerConfig.concurrency`.
    #[tokio::test]
    async fn raising_crawler_concurrency_leaves_rate_limiter_burst_unchanged() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let low = CrawlerConfig::builder(seed.clone()).concurrency(1).build();
        let high = CrawlerConfig::builder(seed).concurrency(16).build();

        let engine_low = Engine::new(low, true).expect("engine must build");
        let engine_high = Engine::new(high, true).expect("engine must build");

        assert_eq!(
            engine_low.budget.burst().get(),
            engine_high.budget.burst().get(),
            "burst must not track crawler concurrency"
        );
        assert!(engine_high.budget.burst().get() > 0);
    }

    /// The RateLimiterConfig construction point derives its burst from the
    /// model: with a FixedDetector of 4 cores the auto table yields crawl
    /// (= default burst) 3, regardless of any crawler concurrency value.
    #[test]
    fn rate_limiter_config_burst_comes_from_budget_model() {
        let detector =
            FixedDetector::with_detection(std::num::NonZeroUsize::new(4).expect("non-zero"), None);
        let budget = BudgetModel::build(BudgetOverrides::default(), &detector);
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).concurrency(16).build();

        let rl_config = rate_limiter_config(&config, &budget);
        assert_eq!(rl_config.delay_ms, config.delay_ms);
        assert_eq!(
            rl_config.concurrency,
            budget.burst().get(),
            "burst must come from the model, not config.concurrency"
        );
        assert_eq!(rl_config.concurrency, 3, "4-core auto table value");
    }

    /// Triangulation sweep: across the whole 1..=32 core range the
    /// construction point emits exactly the model's burst tier, whatever
    /// the configured crawler concurrency claims.
    #[test]
    fn rate_limiter_burst_sweep_matches_model_across_core_counts() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        for cores in 1..=32usize {
            let detector = FixedDetector::with_detection(
                std::num::NonZeroUsize::new(cores).expect("cores non-zero"),
                None,
            );
            let budget = BudgetModel::build(BudgetOverrides::default(), &detector);
            for concurrency in [1usize, 8, 16] {
                let config = CrawlerConfig::builder(seed.clone())
                    .concurrency(concurrency)
                    .build();
                assert_eq!(
                    rate_limiter_config(&config, &budget).concurrency,
                    budget.burst().get(),
                    "burst diverged from model at cores={cores} concurrency={concurrency}"
                );
            }
        }
    }

    /// Bug R2-1: an explicit operator override carried on `CrawlerConfig`
    /// must reach the Engine — `Engine::new` feeds `config.budget_overrides`
    /// into `BudgetModel::build`, so the scheduler spawn bound (Operation.crawl)
    /// and rate-limiter burst reflect the override instead of the auto table.
    #[tokio::test]
    async fn engine_new_consumes_config_budget_overrides() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let overrides = BudgetOverrides {
            crawl: CrawlConcurrency::new(7).ok(),
            rate_burst: BurstPermits::new(9).ok(),
            ..BudgetOverrides::default()
        };
        let config = CrawlerConfig::builder(seed)
            .concurrency(1)
            .budget_overrides(overrides)
            .build();

        let engine = Engine::new(config, true).expect("engine must build");

        assert_eq!(
            engine.budget.crawl().get(),
            7,
            "explicit --concurrency must reach the Engine scheduler bound"
        );
        assert_eq!(
            engine.budget.burst().get(),
            9,
            "explicit --rate-limit-burst must reach the Engine rate limiter"
        );
    }

    /// Triangulation: with default (no-op) overrides the Engine reproduces
    /// exactly the model derived from those same default overrides through
    /// the canonical detector seam — the channel changes nothing when unset.
    #[tokio::test]
    async fn engine_new_default_overrides_keep_auto_derivation() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let expected = BudgetModel::build(
            BudgetOverrides::default(),
            &crate::domain::budget::detector::SystemDetector,
        );
        let config = CrawlerConfig::builder(seed).concurrency(16).build();

        let engine = Engine::new(config, true).expect("engine must build");

        assert_eq!(
            engine.budget.crawl().get(),
            expected.crawl().get(),
            "default overrides must keep the auto-derived crawl tier"
        );
        assert_eq!(
            engine.budget.burst().get(),
            expected.burst().get(),
            "default overrides must keep the auto-derived burst tier"
        );
    }

    /// Shutdown while a worker waits: cancellation must abort the parked
    /// worker and `run()` must return within seconds, not after the 60s
    /// rate-limit refill (#509 acceptance).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_aborts_rate_blocked_worker_and_run_returns_within_bound() {
        let server = MockServer::start().await;
        let port = server.address().port();
        // Burst-decoupled (#302 D1): the token-bucket burst no longer tracks
        // `config.concurrency`, so blocking a worker behind the 60s refill
        // must not depend on burst == 1. Link MORE URLs than the maximum
        // possible burst (ceiling 16): at least one worker necessarily waits
        // for a refill that never comes before cancellation fires.
        let links: String = (0..20)
            .map(|i| format!(r#"<a href=\"http://127.0.0.1:{port}/linked{i}\">next</a>"#))
            .collect();
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body>{links}</body></html>")),
            )
            .mount(&server)
            .await;
        Mock::given(path_regex("/linked\\d+".to_string()))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>linked</body></html>"),
            )
            .mount(&server)
            .await;

        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
        let config = CrawlerConfig::builder(seed)
            .max_depth(1)
            .max_pages(50) // above link count: page count must be bounded by burst, not max_pages
            .concurrency(1)
            .delay_ms(60_000) // 60s refill: without cancellation run() hangs ~1 min
            .timeout_secs(5)
            .ignore_robots(true)
            .build();

        let mut engine = Engine::new(config, true).expect("engine must build");
        let cancel = engine.cancel_handle();

        let run_handle = tokio::spawn(async move { engine.run().await });

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
        // Let immediate-burst workers land so late-discovered workers are
        // genuinely parked on the 60s refill when cancellation fires.
        tokio::time::sleep(Duration::from_millis(100)).await;

        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(10), run_handle)
            .await
            .expect("run() must return within the bound after cancellation")
            .expect("spawned run task must not panic");
        let crawl = result.expect("cancelled crawl must still return a result");

        // #509 acceptance: blocked workers abort as control signals.
        assert_eq!(crawl.errors, 0, "cancelled tasks are control signals");
        // Burst ceiling bounds immediate fetches (seed + <= burst links);
        // the 60s refill means the cancelled run can NEVER fetch all 20.
        let max_burst = crate::domain::budget::clamp::MAX_CONCURRENCY_CEILING;
        assert!(
            crawl.total_pages <= 1 + max_burst,
            "total_pages={} exceeds seed + maximum burst {max_burst}",
            crawl.total_pages
        );
        assert!(
            crawl.total_pages < 20,
            "cancelled run must not fetch all 20 links (60s refill), got {}",
            crawl.total_pages
        );
    }

    /// Regression test for issue #590, bug #1: max_pages MUST be a hard
    /// ceiling. The engine re-checks `collector.is_full()` AFTER
    /// `process_completed_tasks()` and BEFORE `spawn_available_tasks()` so
    /// concurrent completions past the limit cannot fan out more workers.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn max_pages_hard_ceiling_no_overshoot() {
        let server = MockServer::start().await;
        let port = server.address().port();

        // Seed page links to many pages — without the fix, the engine would
        // keep spawning and overshoot max_pages by a wide margin.
        let links: String = (0..20)
            .map(|i| format!(r#"<a href="http://127.0.0.1:{port}/page{i}">link</a>"#))
            .collect();
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(format!("<html><body>{links}</body></html>")),
            )
            .mount(&server)
            .await;

        // Every linked page returns 200 with no further links.
        for i in 0..20 {
            Mock::given(path(format!("/page{i}")))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_string(format!("<html><body>page {i}</body></html>")),
                )
                .mount(&server)
                .await;
        }

        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
        let config = CrawlerConfig::builder(seed)
            .max_depth(2)
            .max_pages(2) // Hard ceiling: must not exceed by much
            .concurrency(5) // High concurrency to trigger the race
            .delay_ms(1)
            .timeout_secs(5)
            .ignore_robots(true)
            .build();

        let mut engine = Engine::new(config, true).expect("engine must build");
        let result = engine.run().await.expect("crawl must complete");
        engine.shutdown().await;

        // The fix guarantees the loop breaks as soon as counter >= max_pages.
        // Up to (spawn bound - 1) in-flight tasks may still land, so we allow
        // a small slack but assert it stays bounded. Since task 2.2b the
        // spawn bound is the model's Operation.crawl tier — the configured
        // `concurrency(5)` no longer gates spawning.
        let crawl_tier = BudgetModel::build(
            BudgetOverrides::default(),
            &crate::domain::budget::detector::SystemDetector,
        )
        .crawl()
        .get();
        assert!(
            result.total_pages <= 2 + crawl_tier,
            "total_pages={} must not overshoot max_pages=2 by more than the \
                 Operation.crawl tier slack ({crawl_tier})",
            result.total_pages
        );
        // Critical: must NOT have fetched all 20 links.
        assert!(
            result.total_pages < 20,
            "must not fetch all 20 links with max_pages=2, got {}",
            result.total_pages
        );
    }

    /// Regression test for #634: seed URL that matches an exclude pattern
    /// MUST be filtered out — 0 pages crawled.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn seed_excluded_by_host_pattern_returns_empty() {
        let server = MockServer::start().await;
        let port = server.address().port();
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"),
            )
            .mount(&server)
            .await;

        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
        let config = CrawlerConfig::builder(seed)
            .max_depth(0)
            .max_pages(10)
            .exclude_pattern("127.0.0.1") // Exclude the seed host
            .ignore_robots(true)
            .build();

        let mut engine = Engine::new(config, true).expect("engine must build");
        let result = engine.run().await.expect("engine run must succeed");
        engine.shutdown().await;

        assert_eq!(
            result.total_pages, 0,
            "seed matching exclude pattern must produce 0 pages, got {}",
            result.total_pages
        );
    }

    /// Regression test for #634: seed URL that does NOT match an exclude
    /// pattern MUST still be crawled (sanity check).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn seed_not_excluded_is_crawled() {
        let server = MockServer::start().await;
        let port = server.address().port();
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"),
            )
            .mount(&server)
            .await;

        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
        let config = CrawlerConfig::builder(seed)
            .max_depth(0)
            .max_pages(10)
            .exclude_pattern("other-host.com") // Does NOT match seed
            .ignore_robots(true)
            .build();

        let mut engine = Engine::new(config, true).expect("engine must build");
        let result = engine.run().await.expect("engine run must succeed");
        engine.shutdown().await;

        assert_eq!(
            result.total_pages, 1,
            "seed NOT matching exclude pattern must be crawled, got {} pages",
            result.total_pages
        );
    }

    // ——— ADR-0012-B sub-slice 3.F: session pool is the domain port (#1075) ———

    /// In-crate fake port — proves `Engine` stores and shares the injected
    /// trait object itself, with no concrete-pool construction in `application`.
    struct FakeSessionPort;

    impl SessionPort for FakeSessionPort {
        fn acquire(&self, _domain: &str) -> Option<crate::domain::session_port::SessionId> {
            Some(crate::domain::session_port::SessionId(0))
        }
        fn report_success(&self, _domain: &str, _session: crate::domain::session_port::SessionId) {}
        fn report_failure(
            &self,
            _domain: &str,
            _session: crate::domain::session_port::SessionId,
            _status: u16,
        ) {
        }
    }

    /// 3.F wiring: `with_session_pool` injects the port, and `build_task_ctx`
    /// hands the SAME `Arc` to the crawl task context (the pre-3.F code cloned
    /// the concrete into a fresh `Arc`; sharing one instance is the point of
    /// the port — ban/cooldown state must be visible to every consumer).
    /// `#[tokio::test]`: `Engine::new` spawns the collector worker.
    #[tokio::test]
    async fn with_session_pool_injects_port_shared_with_task_ctx() {
        let seed = Url::parse("https://example.com").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).max_depth(0).build();
        let pool: Arc<dyn SessionPort> = Arc::new(FakeSessionPort);

        let engine = Engine::new(config, true)
            .expect("engine must build")
            .with_session_pool(Arc::clone(&pool));

        let ctx = engine.build_task_ctx();
        let wired = ctx
            .session_pool
            .as_ref()
            .expect("injected pool must reach the task ctx");
        assert!(
            Arc::ptr_eq(wired, &pool),
            "task ctx must share the injected port instance, not a clone of a concrete"
        );
    }

    /// 3.F end-to-end: the options flow (`session_pool_enabled: true`) builds
    /// the real pool through the composition-root helper
    /// `application::container::build_crawl_session_pool` and the crawl still
    /// completes — the port-gated fetch path must not block a healthy domain.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn crawl_site_with_options_session_pool_enabled_crawls_through_port() {
        let server = MockServer::start().await;
        let port = server.address().port();
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("<html><body>seed</body></html>"),
            )
            .mount(&server)
            .await;

        let seed = Url::parse(&format!("http://127.0.0.1:{port}/")).expect("valid seed URL");
        let config = CrawlerConfig::builder(seed)
            .max_depth(0)
            .max_pages(10)
            .ignore_robots(true)
            .build();
        let options = EngineOptions {
            session_pool_enabled: true,
            ignore_robots: true,
            ..Default::default()
        };

        let result = crawl_site_with_options(config, options)
            .await
            .expect("crawl with session pool enabled must succeed");
        assert_eq!(
            result.total_pages, 1,
            "seed must be crawled through the port-wired pool"
        );
    }

    // ——— PersistenceMode::with_persistence wiring (5c) ———

    #[tokio::test]
    async fn with_persistence_disabled_leaves_checkpoint_disabled() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).build();
        let engine = Engine::new(config, true).expect("engine must build");
        assert_eq!(engine.checkpoint_interval, 100);
        assert!(engine.checkpoint_path.is_none());

        let mode = crate::domain::persistence::PersistenceMode::Disabled;
        let engine = engine.with_persistence(mode);
        assert!(engine.checkpoint_path.is_none());
        // Disabled must not change interval (preserved from initial 100, but not enabling file)
        // The key invariant: no checkpoint file configured.
        assert!(engine.checkpoint_path.is_none());
    }

    #[tokio::test]
    async fn with_persistence_checkpoint_configures_path_and_interval() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).build();
        let engine = Engine::new(config, true).expect("engine must build");

        let tmp = tempfile::TempDir::new().expect("tempdir");
        let mode = crate::domain::persistence::PersistenceMode::Checkpoint {
            cfg: crate::domain::persistence::CheckpointCfg {
                dir: tmp.path().to_path_buf(),
                interval: 42,
            },
        };
        let engine = engine.with_persistence(mode);
        assert!(engine.checkpoint_path.is_some());
        assert_eq!(engine.checkpoint_interval, 42);
        assert!(
            engine.checkpoint_path.unwrap().starts_with(tmp.path()),
            "checkpoint path should be under requested dir"
        );
    }

    #[tokio::test]
    async fn with_persistence_resume_only_leaves_checkpoint_disabled() {
        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).build();
        let engine = Engine::new(config, true).expect("engine must build");

        let mode = crate::domain::persistence::PersistenceMode::Resume {
            dir: std::path::PathBuf::from("/tmp/resume"),
        };
        let engine = engine.with_persistence(mode);
        assert!(engine.checkpoint_path.is_none());
    }

    /// ADR-0012 sub-slice 3.B-1c — the autoscale loop MUST read RAM via the
    /// injected [`RamProbePort`], not via a hardcoded `ResourceGovernor` static
    /// call. This test injects a high-pressure reading, runs the autoscale
    /// background task, and asserts the shared concurrency level reaches
    /// `Critical` without ever touching real sysinfo.
    #[tokio::test]
    async fn with_autoscale_uses_injected_ram_probe() {
        #[derive(Debug)]
        struct HighPressureProbe;
        impl crate::domain::ram_probe_port::RamProbePort for HighPressureProbe {
            fn ram_usage_percent(&self) -> crate::domain::ram_probe_port::RamUsagePercent {
                crate::domain::ram_probe_port::RamUsagePercent::new_clamped(95.0)
            }
        }
        impl crate::domain::ram_probe_port::Sealed for HighPressureProbe {}

        let seed = Url::parse("http://127.0.0.1:9/").expect("valid seed URL");
        let config = CrawlerConfig::builder(seed).build();
        let engine = Engine::new(config, true)
            .expect("engine must build")
            .with_ram_probe(Arc::new(HighPressureProbe))
            .with_autoscale();

        // The probe returns 95% on every poll. The autoscale background loop
        // ticks at 5s; we wait long enough for at least one post-skip tick
        // to land, then verify the shared level moved to Critical. The
        // probe is `Arc<dyn RamProbePort>` so it MUST be the one polled —
        // not the production `SystemRamProbe`.
        let level = engine
            .scheduler
            .autoscale_level()
            .expect("with_autoscale must install a level")
            .clone();
        // Sleep slightly longer than one 5s tick so the spawn'd loop runs
        // at least once past the initial skip-tick.
        tokio::time::sleep(Duration::from_millis(5_100)).await;
        assert_ne!(
            level.get(),
            crate::application::crawler::concurrency_level::ConcurrencyLevel::Normal,
            "autoscale loop must have moved off Normal under 95% injected RAM pressure",
        );
        assert_eq!(
            level.get(),
            crate::application::crawler::concurrency_level::ConcurrencyLevel::Critical,
            "autoscale loop must reach Critical at 95% (>= 90% threshold)",
        );
    }
}
