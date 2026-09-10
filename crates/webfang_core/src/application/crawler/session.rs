//! `CrawlSession` — one validated owner for one crawl run (P6-2/RC-2 slice 1).
//!
//! A crawl run used to have no owner: per-run state was assembled in five
//! places (`Engine::new`, `Engine::build_task_ctx`, the three entry fns, CLI
//! discovery, batch), each re-deriving a subset of the knobs. `CrawlSession`
//! is the single validated run object; [`Engine`](super::engine::Engine) keeps
//! executing and receives a session instead of a knob bag
//! (`Engine::from_session`).
//!
//! Slice 1 is additive and revertible: the entry functions keep their
//! signatures and build a session internally before delegating; `EngineOptions`
//! stays compilable as a transitional view (removed in slice 4).
#![deny(clippy::await_holding_lock)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, RwLock};

use thiserror::Error;
use tokio::sync::RwLock as AsyncRwLock;
use tokio_util::sync::CancellationToken;
use wreq_util::Profile;

use crate::application::crawler::checkpoint::{
    self, BannedDomain, BincodeCheckpoint, CheckpointPath, CheckpointStore, CrawlCheckpoint,
};
use crate::application::crawler::collector::ResultsCollector;
use crate::application::crawler::content_sink::CrawlContentSink;
use crate::application::crawler::crawl_task_ctx::CrawlTaskCtx;
use crate::application::crawler::ports;
use crate::application::pipeline::{OutputStage, PipelineExecutor};
use crate::application::rate_limiter::SharedRateLimiter;
use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::crawler_port::{RobotsPort, UrlQueuePort};
use crate::domain::downloader_factory::DownloaderFactory;
use crate::domain::downloader_port::Downloader;
use crate::domain::persistence::PersistenceMode;
use crate::domain::session_port::SessionPort;
use crate::domain::{CorrelationId, CrawlerConfig, JsStrategy};
use crate::error::ScraperError;
use crate::infrastructure::observability::log_scrape_error;

/// Identity minted once per run and shared by every unit of work.
///
/// The root [`CorrelationId`] is never persisted and never joined across runs
/// (AGENTS.md: `trace_id` is identity-within-run); `run_label` is a read-only
/// human tag for logs and traces only.
#[derive(Debug, Clone)]
pub(crate) struct CrawlIdentity {
    /// Run-root identity — every page derives a child from it.
    pub root: CorrelationId,
    /// Human-readable run tag (seed host by default); logs/traces only.
    pub run_label: String,
}

/// Persistence input for one run (D4: [`PersistenceMode`] is the SOLE input —
/// no independent checkpoint path/interval setters, the F-01/#1214 bug class).
#[derive(Debug, Clone)]
pub(crate) struct PersistencePolicy {
    /// Resolved persistence mode (pure resolver output, no IO).
    pub mode: PersistenceMode,
    /// Checkpoint loaded by [`CrawlSession::begin`] (`None` = fresh start).
    pub loaded: Option<CrawlCheckpoint>,
}

/// Transport policy for one run: how pages are fetched.
#[derive(Debug, Clone)]
pub(crate) struct TransportPolicy {
    /// Rendering strategy selecting the downloader stack.
    pub js_strategy: JsStrategy,
    /// TLS/HTTP2 fingerprint profile for the wreq layer.
    pub tls_emulation: Profile,
    /// Bypass WAF classification on the hybrid spa-detection path.
    pub ignore_waf: bool,
    /// Maximum retry attempts / backoff bounds.
    pub max_retries: u32,
    /// Base delay for exponential backoff (ms).
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff (ms).
    pub backoff_max_ms: u64,
    /// Hybrid Layer 2 binary name or path (#787).
    pub obscura_binary: String,
    /// Gate-certified Chrome binary (F-52-c, #1278). `None` keeps launcher
    /// auto-detection. Threaded into `with_js_strategy` like every other
    /// transport knob (added on rebase over 08eee306; `post_load_wait`
    /// follows after F-52-b merges).
    pub chrome_binary: Option<std::path::PathBuf>,
    /// Domain session pool enabled (pool itself arrives via [`CrawlPorts`]).
    pub session_pool_enabled: bool,
    /// Autoscaled concurrency from system RAM.
    pub autoscale_enabled: bool,
    /// Skip robots.txt enforcement.
    pub ignore_robots: bool,
}

/// Injected run seams: every trait object the run needs beyond configuration.
///
/// Concrete construction stays in `application::container` (D8) — the session
/// only carries `Arc<dyn …>` handles assembled by the caller.
#[derive(Clone)]
pub(crate) struct CrawlPorts {
    /// Domain session pool (built by the caller when enabled).
    pub session_pool: Option<Arc<dyn SessionPort>>,
    /// Factory building the fetch downloader for the strategy.
    pub downloader_factory: Option<Arc<dyn DownloaderFactory>>,
    /// Sink capturing every fetched page body.
    pub content_sink: Option<Arc<dyn CrawlContentSink>>,
    /// Item pipeline executor.
    pub pipeline: Option<Arc<PipelineExecutor>>,
    /// Output stages receiving items after pipeline processing.
    pub output_stages: Vec<Arc<Box<dyn OutputStage>>>,
}

/// Engine-owned execution handles consumed by [`CrawlSession::task_ctx`].
///
/// The engine keeps building its scheduler, limiter, counters, collector,
/// bridges and fetch router exactly as today; this bundle hands them to the
/// session so the shared task context is *derived* from run facts instead of
/// re-derived by the executor (D6). Assembled by `Engine::build_task_ctx`.
pub(crate) struct CrawlExec {
    /// Shared discovery queue.
    pub queue: Arc<dyn UrlQueuePort>,
    /// Shared rate limiter.
    pub rate_limiter: SharedRateLimiter,
    /// Total pages crawled.
    pub pages_crawled: Arc<AtomicU64>,
    /// Fetch failures observed.
    pub error_count: Arc<AtomicUsize>,
    /// Per-category error counters.
    pub error_breakdown: Arc<[AtomicUsize; 8]>,
    /// Results collector (cloned; the engine keeps draining its own handle).
    pub collector: ResultsCollector,
    /// Shared cookie jar.
    pub cookie_bridge: Arc<AsyncRwLock<CookieBridge>>,
    /// WAF/rate-limit banned domains.
    pub banned_domains: Arc<RwLock<Vec<BannedDomain>>>,
    /// TLS-fingerprinted robots.txt fetcher.
    pub robots_fetcher: Arc<dyn RobotsPort>,
    /// Strategy downloader (`None` = static fallback, as today).
    pub fetch_router: Option<Arc<dyn Downloader>>,
}

/// One validated crawl run: identity, config, policies and ports.
///
/// Immutable after [`CrawlSessionBuilder::build`]; the run lifecycle is
/// `begin()` → execute (`Engine::from_session` + `run`) → `finish()` (D7:
/// consuming, `#[must_use]` — a session that is never run leaks its ports).
#[must_use = "a crawl session that is never run leaks its ports"]
#[derive(Clone)]
pub(crate) struct CrawlSession {
    /// Run identity (single-minted root + log label).
    pub(crate) identity: CrawlIdentity,
    /// Target configuration (seed, depth, patterns, budgets).
    pub(crate) config: Arc<CrawlerConfig>,
    /// Persistence input + loaded resume state.
    pub(crate) persistence: PersistencePolicy,
    /// Fetch policy.
    pub(crate) transport: TransportPolicy,
    /// Injected seams.
    pub(crate) ports: CrawlPorts,
    /// Single cancellation authority for the run (#509).
    pub(crate) cancel_token: CancellationToken,
}

/// Outcome of [`CrawlSession::begin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BeginOutcome {
    /// Whether a persisted checkpoint was restored.
    pub resumed: bool,
    /// Whether checkpointing degraded to disabled (unwritable dir).
    pub degraded: bool,
}

/// Verdict of [`CrawlSession::finish`] over the checkpoint file.
///
/// Derived internally by `finish` (P6-2 slice 2) and surfaced in
/// [`SessionClose`]. Mirrors today's F-01 branches exactly
/// (completed → delete, interrupted → save, disabled → skip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CheckpointAction {
    /// Run completed fully: delete the checkpoint (no stale resume).
    Delete,
    /// Run interrupted or truncated: (re)write the checkpoint.
    Write,
    /// Checkpointing disabled: nothing to do.
    Skip,
}

/// Outcome of [`CrawlSession::finish`]: the close facts the engine needs
/// for its close trace event. The checkpoint IO already happened — the
/// engine only reacts by clearing its own IO handle on [`CheckpointAction::Delete`]
/// so its shutdown save cannot re-create the removed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionClose {
    /// Human-readable run tag (seed host by default).
    pub run_label: String,
    /// Verdict executed on the checkpoint file.
    pub action: CheckpointAction,
}

/// Application-layer session failures (D-spec error stratification).
///
/// Maps into the existing chain via `From<CrawlSessionError> for ScraperError`
/// — no new level (the `AppError` tier in AGENTS.md/README.md has zero
/// production hits; inventing it here would match a stale doc). Every variant
/// gains a row in `docs/error-classification-matrix.md` in the same change.
#[derive(Debug, Error)]
pub(crate) enum CrawlSessionError {
    /// Run description failed validation — fail before any worker spawns.
    #[error("invalid crawl session: {0}")]
    InvalidConfiguration(String),
    /// Checkpoint directory unwritable — degrades to no-checkpoint (logged).
    #[error("checkpoint unwritable at {path}: {reason}")]
    CheckpointUnwritable {
        /// Directory that could not be created.
        path: PathBuf,
        /// OS error text.
        reason: String,
    },
    /// Run cancelled through the session authority (surfaces only if a
    /// future slice lets cancellation escape engine control flow; today it
    /// is consumed there first — same defensive precedent as
    /// `CrawlError::Cancelled`).
    #[error("crawl cancelled")]
    #[allow(dead_code)] // slice 2: constructed by the shutdown-verdict path
    Cancelled,
    /// Session invariant broken (bug indicator).
    #[error("crawl session internal error: {0}")]
    #[allow(dead_code)] // slice 2: constructed by future validation branches
    Internal(String),
}

impl From<CrawlSessionError> for ScraperError {
    fn from(err: CrawlSessionError) -> Self {
        match err {
            // PermanentFatal / exit 78 via the Config row (matrix family 4).
            CrawlSessionError::InvalidConfiguration(msg) => ScraperError::Config(msg),
            CrawlSessionError::CheckpointUnwritable { path, reason } => ScraperError::Config(
                format!("checkpoint unwritable at {}: {reason}", path.display()),
            ),
            // Defensive only: cancellation is consumed by engine control flow
            // before any error surfaces (same precedent as CrawlError::Cancelled).
            CrawlSessionError::Cancelled => {
                ScraperError::Internal("crawl cancelled by session authority".to_string())
            },
            CrawlSessionError::Internal(msg) => ScraperError::Internal(msg),
        }
    }
}

/// Builder for [`CrawlSession`] — the only construction site (D1: one place
/// per run setting; adding a knob means adding one builder field).
#[derive(Default)]
pub(crate) struct CrawlSessionBuilder {
    config: Option<CrawlerConfig>,
    persistence: Option<PersistenceMode>,
    transport: Option<TransportPolicy>,
    ports: Option<CrawlPorts>,
    identity: Option<CrawlIdentity>,
}

impl CrawlSessionBuilder {
    /// Target configuration (required).
    pub(crate) fn config(mut self, config: CrawlerConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Persistence input (required; D4: the resolved mode, nothing else).
    pub(crate) fn persistence(mut self, mode: PersistenceMode) -> Self {
        self.persistence = Some(mode);
        self
    }

    /// Fetch policy (required).
    pub(crate) fn transport(mut self, policy: TransportPolicy) -> Self {
        self.transport = Some(policy);
        self
    }

    /// Injected seams (required; `None` members mean historical defaults).
    pub(crate) fn ports(mut self, ports: CrawlPorts) -> Self {
        self.ports = Some(ports);
        self
    }

    /// Run identity override (optional; a fresh root is minted otherwise —
    /// single mint per run, never shared across runs).
    pub(crate) fn identity(mut self, identity: CrawlIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Validate the assembled description completely before any worker spawns.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlSessionError::InvalidConfiguration`] when a required
    /// part is missing, the seed scheme is not fetchable, the backoff bounds
    /// are inverted, or a checkpoint target carries a zero interval.
    pub(crate) fn build(self) -> Result<CrawlSession, CrawlSessionError> {
        let config = self.config.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing target configuration".to_string())
        })?;
        if !matches!(config.seed_url.scheme(), "http" | "https") {
            return Err(CrawlSessionError::InvalidConfiguration(format!(
                "seed scheme '{}' is not fetchable (http/https required)",
                config.seed_url.scheme()
            )));
        }
        let persistence = self.persistence.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing persistence policy".to_string())
        })?;
        if let Some(interval) = checkpoint_interval_of(&persistence) {
            if interval == 0 {
                return Err(CrawlSessionError::InvalidConfiguration(
                    "checkpoint target set with zero interval".to_string(),
                ));
            }
        }
        let transport = self.transport.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing transport policy".to_string())
        })?;
        if transport.backoff_max_ms < transport.backoff_base_ms {
            return Err(CrawlSessionError::InvalidConfiguration(format!(
                "backoff_max_ms ({}) < backoff_base_ms ({})",
                transport.backoff_max_ms, transport.backoff_base_ms
            )));
        }
        let ports = self.ports.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing port bundle".to_string())
        })?;
        let identity = self.identity.unwrap_or_else(|| CrawlIdentity {
            root: CorrelationId::new(),
            run_label: config.seed_url.host_str().unwrap_or("seed").to_string(),
        });
        Ok(CrawlSession {
            identity,
            config: Arc::new(config),
            persistence: PersistencePolicy {
                mode: persistence,
                loaded: None,
            },
            transport,
            ports,
            cancel_token: CancellationToken::new(),
        })
    }
}

/// Checkpoint interval carried by a mode (`None` = persistence off).
fn checkpoint_interval_of(mode: &PersistenceMode) -> Option<u64> {
    match mode {
        PersistenceMode::Disabled | PersistenceMode::Resume { .. } => None,
        PersistenceMode::Checkpoint { cfg }
        | PersistenceMode::Full {
            checkpoint: cfg, ..
        } => Some(cfg.interval),
    }
}

impl CrawlSession {
    /// New builder (the only construction path).
    pub(crate) fn builder() -> CrawlSessionBuilder {
        CrawlSessionBuilder::default()
    }

    /// Run identity (root correlation + log label).
    pub(crate) fn identity(&self) -> &CrawlIdentity {
        &self.identity
    }

    /// Target configuration.
    pub(crate) fn config(&self) -> &Arc<CrawlerConfig> {
        &self.config
    }

    /// Single cancellation authority for the run (#509).
    pub(crate) fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// Cancellation token clone for engine adoption (`from_session`).
    pub(crate) fn cancel_token(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Open the run: ensure the checkpoint directory, load resume state, and
    /// emit the run-identity event. Runs once, before any worker spawns.
    ///
    /// An unwritable directory degrades to no-checkpoint (logged through the
    /// shared error path) instead of failing the run — the crawl still
    /// produces its result.
    pub(crate) fn begin(&mut self) -> BeginOutcome {
        let seed = self.config.seed_url.as_str().to_string();
        tracing::info!(
            correlation_id = %self.identity.root,
            trace_id = %self.identity.root.trace_id(),
            run_label = %self.identity.run_label,
            seed_url = %seed,
            "run identity",
        );
        let dir = match &self.persistence.mode {
            PersistenceMode::Checkpoint { cfg }
            | PersistenceMode::Full {
                checkpoint: cfg, ..
            } => Some(cfg.dir.clone()),
            PersistenceMode::Disabled | PersistenceMode::Resume { .. } => None,
        };
        let Some(dir) = dir else {
            return BeginOutcome {
                resumed: false,
                degraded: false,
            };
        };
        let cp_path = CheckpointPath::new(&dir);
        if let Err(e) = cp_path.ensure_dir() {
            let degraded = CrawlSessionError::CheckpointUnwritable {
                path: dir.clone(),
                reason: e,
            };
            log_scrape_error(
                &degraded,
                &seed,
                "session",
                Some(&self.identity.root),
                "checkpoint dir creation failed — disabling checkpoint",
            );
            self.persistence.mode = PersistenceMode::Disabled;
            return BeginOutcome {
                resumed: false,
                degraded: true,
            };
        }
        let scoped = cp_path.file_for_seed(&seed);
        match BincodeCheckpoint::new().load(&scoped) {
            Some(cp) => {
                let outcome = BeginOutcome {
                    resumed: true,
                    degraded: false,
                };
                tracing::info!(
                    correlation_id = %self.identity.root,
                    visited = cp.visited.len(),
                    pages = cp.pages_crawled,
                    "resuming from checkpoint",
                );
                self.persistence.loaded = Some(cp);
                outcome
            },
            None => {
                tracing::info!("no checkpoint found, starting fresh");
                BeginOutcome {
                    resumed: false,
                    degraded: false,
                }
            },
        }
    }

    /// Derive the shared task context from run facts + engine execution
    /// handles (D6). Built once per run; the wrapper set is a run fact, not
    /// an execution fact — the engine must not rebuild it per call site.
    pub(crate) fn task_ctx(&self, exec: CrawlExec) -> Arc<CrawlTaskCtx> {
        Arc::new(CrawlTaskCtx {
            config: Arc::clone(&self.config),
            correlation_id: self.identity.root.clone(),
            queue: exec.queue,
            rate_limiter: exec.rate_limiter,
            cancel_token: self.cancel_token.clone(),
            session_pool: self.ports.session_pool.clone(),
            ignore_robots: self.transport.ignore_robots,
            robots_checker: Arc::new(ports::ProductionRobotsChecker {
                fetcher: exec.robots_fetcher,
            }),
            error_count: exec.error_count,
            error_breakdown: exec.error_breakdown,
            pages_crawled: exec.pages_crawled,
            collector: Arc::new(ports::ProductionCollector {
                collector: exec.collector,
            }),
            cookie_bridge: exec.cookie_bridge,
            banned_domains: exec.banned_domains,
            fetcher: Arc::new(ports::ProductionPageFetcher {
                router: exec.fetch_router.clone(),
                fallback: crate::application::container::build_static_fetcher(),
            }),
            link_extractor: Arc::new(ports::ProductionLinkExtractor::new(
                crate::application::container::build_link_extractor(),
            )),
            content_sink: self.ports.content_sink.clone(),
            pipeline: self.ports.pipeline.as_ref().map(|p| {
                Arc::new(ports::ProductionPipeline {
                    executor: Arc::clone(p),
                }) as Arc<dyn ports::ContentPipeline>
            }),
            output_stages: self.ports.output_stages.clone(),
        })
    }

    /// End the run: derive the F-01 verdict AND perform the checkpoint IO
    /// (P6-2 slice 2 — close-time IO ownership lives here; the engine no
    /// longer writes checkpoint state at close).
    ///
    /// Verdict table unchanged from slice 1: completed + enabled → delete,
    /// interrupted + enabled → write, disabled → skip. The write path invokes
    /// `checkpoint_snapshot` exactly once — only a Write pays for the crawl
    /// state snapshot; delete and skip never touch it. The scoped checkpoint
    /// path is derived from the persistence policy, so closing does not depend
    /// on `begin()` having run (a degraded `begin` already flipped the mode to
    /// [`PersistenceMode::Disabled`], which skips here too).
    ///
    /// Returns the run label + action for the engine's close trace event (the
    /// event itself stays in the engine: its `crawl completed` summary is the
    /// benchmark aggregator key and must not grow a sibling).
    pub(crate) async fn finish(
        self,
        completed_fully: bool,
        checkpoint_snapshot: impl AsyncFnOnce() -> CrawlCheckpoint,
    ) -> SessionClose {
        let scoped_path =
            checkpoint_scoped_path(&self.persistence.mode, self.config.seed_url.as_str());
        let action = match (completed_fully, scoped_path.is_some()) {
            (true, true) => {
                if let Some(path) = &scoped_path {
                    checkpoint::delete_checkpoint_file(path);
                }
                CheckpointAction::Delete
            },
            (false, true) => {
                if let Some(path) = scoped_path {
                    let state = checkpoint_snapshot().await;
                    let outcome =
                        checkpoint::persist_checkpoint_state(BincodeCheckpoint::new(), state, path)
                            .await;
                    checkpoint::log_checkpoint_save(outcome);
                }
                CheckpointAction::Write
            },
            (_, false) => CheckpointAction::Skip,
        };
        SessionClose {
            run_label: self.identity.run_label,
            action,
        }
    }
}

/// Scoped checkpoint file path for the run's seed, when checkpointing is
/// enabled (`None` = checkpointing off — same derivation as `begin()` and
/// `Engine::from_session`; single source of truth for the file location).
fn checkpoint_scoped_path(mode: &PersistenceMode, seed: &str) -> Option<PathBuf> {
    match mode {
        PersistenceMode::Checkpoint { cfg }
        | PersistenceMode::Full {
            checkpoint: cfg, ..
        } => Some(CheckpointPath::new(&cfg.dir).file_for_seed(seed)),
        PersistenceMode::Disabled | PersistenceMode::Resume { .. } => None,
    }
}

/// Transitional shim: [`TransportPolicy`] from [`EngineOptions`].
///
/// Lets slice-1 callers migrate one knob bag at a time; `EngineOptions`
/// deprecation (slice 4) removes it.
impl From<&crate::application::crawler::engine::EngineOptions> for TransportPolicy {
    fn from(options: &crate::application::crawler::engine::EngineOptions) -> Self {
        Self {
            js_strategy: options.js_strategy,
            tls_emulation: options.tls_emulation,
            ignore_waf: options.ignore_waf,
            max_retries: options.max_retries,
            backoff_base_ms: options.backoff_base_ms,
            backoff_max_ms: options.backoff_max_ms,
            obscura_binary: options.obscura_binary.clone(),
            chrome_binary: options.chrome_binary.clone(),
            session_pool_enabled: options.session_pool_enabled,
            autoscale_enabled: options.autoscale_enabled,
            ignore_robots: options.ignore_robots,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::persistence::CheckpointCfg;

    fn test_config() -> CrawlerConfig {
        CrawlerConfig::builder(url::Url::parse("https://example.com").expect("seed")).build()
    }

    fn test_transport() -> TransportPolicy {
        TransportPolicy {
            js_strategy: JsStrategy::Static,
            tls_emulation: wreq_util::Profile::Chrome145,
            ignore_waf: false,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            obscura_binary: "obscura".to_string(),
            chrome_binary: None,
            session_pool_enabled: false,
            autoscale_enabled: false,
            ignore_robots: false,
        }
    }

    fn test_ports() -> CrawlPorts {
        CrawlPorts {
            session_pool: None,
            downloader_factory: None,
            content_sink: None,
            pipeline: None,
            output_stages: Vec::new(),
        }
    }

    fn build_ok(mode: PersistenceMode) -> CrawlSession {
        CrawlSession::builder()
            .config(test_config())
            .persistence(mode)
            .transport(test_transport())
            .ports(test_ports())
            .build()
            .expect("valid session must build")
    }

    #[test]
    fn build_requires_every_part() {
        assert!(
            matches!(
                CrawlSession::builder().build(),
                Err(CrawlSessionError::InvalidConfiguration(_))
            ),
            "empty builder must fail"
        );
    }

    #[test]
    fn build_rejects_non_fetchable_seed() {
        let config =
            CrawlerConfig::builder(url::Url::parse("ftp://example.com/x").expect("seed")).build();
        let result = CrawlSession::builder()
            .config(config)
            .persistence(PersistenceMode::Disabled)
            .transport(test_transport())
            .ports(test_ports())
            .build();
        assert!(
            matches!(result, Err(CrawlSessionError::InvalidConfiguration(_))),
            "ftp seed must fail"
        );
    }

    #[test]
    fn build_rejects_inverted_backoff() {
        let mut transport = test_transport();
        transport.backoff_base_ms = 5000;
        transport.backoff_max_ms = 1000;
        let result = CrawlSession::builder()
            .config(test_config())
            .persistence(PersistenceMode::Disabled)
            .transport(transport)
            .ports(test_ports())
            .build();
        assert!(
            matches!(result, Err(CrawlSessionError::InvalidConfiguration(_))),
            "inverted backoff must fail"
        );
    }

    #[test]
    fn build_rejects_zero_checkpoint_interval() {
        let mode = PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: std::path::PathBuf::from("/tmp/x"),
                interval: 0,
            },
        };
        let result = CrawlSession::builder()
            .config(test_config())
            .persistence(mode)
            .transport(test_transport())
            .ports(test_ports())
            .build();
        assert!(
            matches!(result, Err(CrawlSessionError::InvalidConfiguration(_))),
            "zero interval with target must fail"
        );
    }

    #[tokio::test]
    async fn identity_single_mint_and_default_label() {
        let a = build_ok(PersistenceMode::Disabled);
        let b = build_ok(PersistenceMode::Disabled);
        assert_ne!(
            a.identity().root,
            b.identity().root,
            "two builds must mint distinct roots"
        );
        assert_eq!(a.identity().run_label, "example.com");
        // Consume: an un-run session is a must_use leak by design (D7).
        let _ = a.finish(true, async || CrawlCheckpoint::new()).await;
        let _ = b.finish(true, async || CrawlCheckpoint::new()).await;
    }

    #[tokio::test]
    async fn begin_disabled_is_fresh_without_io() {
        let mut session = build_ok(PersistenceMode::Disabled);
        let outcome = session.begin();
        assert_eq!(
            outcome,
            BeginOutcome {
                resumed: false,
                degraded: false,
            }
        );
        let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
    }
    
    #[tokio::test]
    async fn begin_checkpoint_without_file_starts_fresh() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let mode = PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: tmp.path().to_path_buf(),
                interval: 100,
            },
        };
        let mut session = build_ok(mode);
        let outcome = session.begin();
        assert_eq!(
            outcome,
            BeginOutcome {
                resumed: false,
                degraded: false,
            }
        );
        let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
    }
    
    #[tokio::test]
    async fn begin_unwritable_dir_degrades_without_failing() {
        // A regular file where the directory should be: ensure_dir fails
        // deterministically on every platform (no chmod games).
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"x").expect("blocker file");
        let mode = PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: blocker.join("subdir"),
                interval: 100,
            },
        };
        let mut session = build_ok(mode);
        let outcome = session.begin();
        assert_eq!(
            outcome,
            BeginOutcome {
                resumed: false,
                degraded: true,
            }
        );
        let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
    }
    
    #[tokio::test]
    async fn begin_resumes_matching_checkpoint() {
        use crate::application::crawler::checkpoint::{
            BincodeCheckpoint, CheckpointPath, CheckpointStore,
        };

        let tmp = tempfile::TempDir::new().expect("tempdir");
        // NOTE: `Url::parse` normalizes the seed with a trailing slash — the
        // scoped filename hashes the normalized form, so the fixture must use
        // it too (same string `begin()` derives from the config).
        let seed = "https://example.com/";
        let scoped = CheckpointPath::new(tmp.path()).file_for_seed(seed);
        // CheckpointPath scopes per-seed; ensure the dir the production way.
        CheckpointPath::new(tmp.path())
            .ensure_dir()
            .expect("ensure dir");
        BincodeCheckpoint::new()
            .save(&CrawlCheckpoint::new(), &scoped)
            .expect("seed checkpoint");
        let mode = PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: tmp.path().to_path_buf(),
                interval: 100,
            },
        };
        let mut session = build_ok(mode);
        let outcome = session.begin();
        assert_eq!(
            outcome,
            BeginOutcome {
                resumed: true,
                degraded: false,
            }
        );
        let _ = session.finish(true, async || CrawlCheckpoint::new()).await;
    }
    
    #[tokio::test]
    async fn finish_decision_table() {
        // (completed, enabled) -> action; mirrors today's F-01 branches.
        // The Write case performs real IO — the fixture uses an ephemeral
        // TempDir, never a shared path.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let table = [
            (true, true, CheckpointAction::Delete),
            (false, true, CheckpointAction::Write),
            (true, false, CheckpointAction::Skip),
            (false, false, CheckpointAction::Skip),
        ];
        for (completed, enabled, expected) in table {
            let mode = if enabled {
                PersistenceMode::Checkpoint {
                    cfg: CheckpointCfg {
                        dir: tmp.path().to_path_buf(),
                        interval: 100,
                    },
                }
            } else {
                PersistenceMode::Disabled
            };
            let session = build_ok(mode);
            let close = session
                .finish(completed, async || CrawlCheckpoint::new())
                .await;
            assert_eq!(close.action, expected);
        }
    }

    #[test]
    fn error_maps_into_existing_chain() {
        let err: ScraperError = CrawlSessionError::InvalidConfiguration("bad".to_string()).into();
        assert!(matches!(err, ScraperError::Config(_)));
        let err: ScraperError = CrawlSessionError::CheckpointUnwritable {
            path: std::path::PathBuf::from("/nope"),
            reason: "denied".to_string(),
        }
        .into();
        assert!(matches!(err, ScraperError::Config(_)));
        let err: ScraperError = CrawlSessionError::Internal("bug".to_string()).into();
        assert!(matches!(err, ScraperError::Internal(_)));
    }
}
