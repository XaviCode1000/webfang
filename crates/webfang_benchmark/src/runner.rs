//! In-process runner: drives the engine per [`JsStrategy`] against the Tier A
//! corpus (FR-2, design §2).
//!
//! Sequential deterministic order `[Static, Hybrid, Full]`. Each run gets a
//! fresh corpus server (fresh WAF sequence), a fresh `TempDir` trace file, and
//! a thread-scoped tracing dispatcher (`FileTraceLayer` + `Dispatch::new` +
//! `with_default`, mirroring `benches/tracing_overhead.rs`) around a
//! **current_thread** runtime. Never `set_global_default` — it is
//! once-per-process and would couple runs (ADR-B2).
//!
//! ADR-B2 escape hatch (declared up front, do NOT improvise): `with_default`
//! is thread-scoped, so spans emitted off the runtime thread never reach the
//! trace. The metrics we consume originate from engine spans polled on the
//! runtime thread; if T9's e2e smoke test shows the engine summary line
//! (`crawl completed`) missing due to off-thread emission, fall back to a tiny
//! internal `[[bin]] bench_run --strategy <s>` executed via
//! `std::process::Command` — never weaken the test.

use std::sync::{Arc, Mutex, OnceLock};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;
use url::Url;

use crate::aggregate::{self, CrawlSummary, StrategyMetrics};
use crate::corpus::{self, CorpusManifest};
use crate::error::{BenchmarkError, Result};
use webfang_core::application::crawl_site_with_options;
use webfang_core::application::crawler::EngineOptions;
use webfang_core::domain::cookie_bridge::CookieBridge;
use webfang_core::domain::downloader_factory::{DownloaderFactory, DownloaderSpec};
use webfang_core::domain::value_objects::CorrelationId;
use webfang_core::domain::CrawlerConfig;
use webfang_core::domain::JsStrategy;
use webfang_core::infrastructure::downloader::fetch_router::DefaultDownloaderFactory;
use webfang_core::infrastructure::observability::FileTraceLayer;

/// Deterministic strategy order for every benchmark invocation.
const STRATEGIES: [JsStrategy; 3] = [JsStrategy::Static, JsStrategy::Hybrid, JsStrategy::Full];

/// Process-wide exclusion around every crawl whose strategy can launch the
/// headless browser (`Hybrid` escalates via the SPA detector; `Full` always).
///
/// `ChromiumoxideDownloader` launches Chrome against the fixed profile dir
/// `/tmp/chromiumoxide-runner`; a concurrent launch hits Chrome's
/// `ProcessSingleton` lock (`Failed to create SingletonLock: File exists`) and
/// aborts, which surfaces downstream as an all-failures crawl (`total_pages=0`
/// → [`BenchmarkError::EmptyCrawl`]). Serializing browser-capable runs inside
/// this process removes the race WITHOUT touching core (NFR-2); benchmark
/// runs are sequential by design anyway (ADR-B2).
fn chrome_profile_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// One completed strategy run: computed metrics ready for aggregation.
#[derive(Debug)]
pub struct RunOutcome {
    pub strategy: JsStrategy,
    pub metrics: StrategyMetrics,
    /// Engine summary lifted from this run's own JSONL trace — its presence
    /// proves the `crawl completed` line reached the file (ADR-B2/R2 tripwire).
    pub summary: CrawlSummary,
}

/// Run all strategies sequentially over the corpus and return their metrics.
///
/// Each run serves a fresh corpus (fresh WAF sequence — still deterministic by
/// construction, AC-1.1), writes its own JSONL trace under a private
/// [`tempfile::TempDir`], parses it, computes [`StrategyMetrics`] with the RAM
/// proxy captured from a mirror downloader build
/// ([`memory_cost`](webfang_core::infrastructure::downloader::Downloader::memory_cost)
/// via [`DefaultDownloaderFactory`]), and drops the TempDir before
/// returning so absolute paths can never reach compared output.
///
/// # Errors
///
/// - [`BenchmarkError::Corpus`] if the corpus base URL is not parseable.
/// - [`BenchmarkError::Engine`] if the crawl or the RAM-proxy router build fails.
/// - Parser/compute errors propagate from [`crate::aggregate`].
pub fn run_all(manifest: &CorpusManifest) -> Result<Vec<RunOutcome>> {
    let mut outcomes = Vec::with_capacity(STRATEGIES.len());
    for &strategy in &STRATEGIES {
        outcomes.push(run_strategy(strategy, manifest, CorrelationId::new())?);
    }
    Ok(outcomes)
}

#[tracing::instrument(skip_all, fields(correlation_id = %correlation_id))]
fn run_strategy(
    strategy: JsStrategy,
    manifest: &CorpusManifest,
    correlation_id: CorrelationId,
) -> Result<RunOutcome> {
    tracing::debug!(correlation_id = %correlation_id, "strategy run starting");

    // ONE current_thread runtime per run: it must outlive the corpus server
    // (wiremock tasks die with their runtime) and drive the whole crawl.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let tmp = tempfile::TempDir::new()?;
    let trace_path = tmp.path().join("trace.jsonl");

    // Exact subscriber recipe from benches/tracing_overhead.rs:58-60.
    let layer = FileTraceLayer::new(trace_path.clone())?;
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    // Small page budget: the corpus has `manifest.pages.len()` distinct paths.
    let max_pages = manifest.pages.len();

    let options = EngineOptions {
        js_strategy: strategy,
        ignore_robots: true,
        // ADR-0012 sub-slice 3.B-1b: the engine no longer builds its own
        // fetch downloader. Without the factory the Hybrid/Full runs would
        // silently degrade to the static fetch path and measure the wrong
        // stack, so the benchmark injects the production factory.
        downloader_factory: Some(Arc::new(DefaultDownloaderFactory)),
        ..EngineOptions::default()
    };

    let ram_cost_bytes = ram_proxy(strategy)?;

    // Browser-capable strategies (Hybrid escalates via SPA detection; Full
    // always) hold the process-wide Chrome profile lock for the whole crawl
    // AND get one EmptyCrawl retry: chromiumoxide 0.7 pins every launch to
    // the fixed profile dir `/tmp/chromiumoxide-runner`, so a launch that
    // overlaps another process's Chrome shutdown loses Chrome's
    // ProcessSingleton race and aborts, yielding a `total_pages=0` crawl.
    // One measured retry after a short backoff makes the benchmark robust
    // to that environment contention without touching core (NFR-2) and
    // without altering any successful run's numbers (NFR-1).
    let browser_capable = matches!(strategy, JsStrategy::Hybrid | JsStrategy::Full);
    let measure = |options: EngineOptions| -> Result<RunOutcome> {
        // Fresh trace per attempt: FileTraceLayer truncates on open, so a
        // retry cannot observe the failed attempt's records.
        let _guard = if browser_capable {
            let guard = chrome_profile_lock().lock().map_err(|poisoned| {
                BenchmarkError::Engine(format!("chrome profile lock poisoned: {poisoned}"))
            })?;
            Some(guard)
        } else {
            None
        };
        // FRESH CORPUS PER ATTEMPT (NFR-1): the simulated-WAF sequence is
        // stateful (403→429→200 atomic counter). A retry against an already
        // consumed sequence would observe different responses than any
        // first attempt, breaking byte-identical Tier A reproducibility.
        // Each measurement therefore serves its own corpus instance.
        let handle = rt.block_on(corpus::serve())?;
        let seed = Url::parse(&format!("{}/", handle.base_url)).map_err(|source| {
            BenchmarkError::Corpus(format!("invalid corpus base url: {source}"))
        })?;
        let config = CrawlerConfig::builder(seed)
            .max_pages(max_pages)
            .max_depth(1)
            .concurrency(std::num::NonZeroUsize::new(2).expect("2 is non-zero"))
            .delay_ms(0)
            .build();
        let _crawl_result = tracing::dispatcher::with_default(&dispatch, || {
            rt.block_on(crawl_site_with_options(config.clone(), options))
        })
        .map_err(|error| BenchmarkError::Engine(error.to_string()))?;
        // Post-crawl grace INSIDE the lock: give the previous Chrome
        // process time to fully exit (and release its ProcessSingleton
        // lock on the fixed profile dir) before the next browser launch.
        if browser_capable {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        // `dispatch` dropped above ⇒ FileTraceLayer flushed its buffer to
        // disk before parsing reads the file.
        let records = aggregate::parse_file(&trace_path)?;
        let summary = aggregate::summary_of(&records, "<run trace>")?.clone();
        let metrics = aggregate::compute(&records, strategy, ram_cost_bytes)?;
        Ok(RunOutcome {
            strategy,
            metrics,
            summary,
        })
    };

    let outcome = match measure(options.clone()) {
        Ok(outcome) => outcome,
        Err(BenchmarkError::EmptyCrawl) if browser_capable => {
            let mut outcome = None;
            for attempt in 1..=2 {
                tracing::warn!(
                    strategy = %strategy,
                    attempt = attempt,
                    "zero-page crawl on browser strategy (Chrome ProcessSingleton contention on fixed profile dir); retrying after backoff"
                );
                std::thread::sleep(std::time::Duration::from_millis(500));
                match measure(options.clone()) {
                    Ok(o) => {
                        outcome = Some(o);
                        break;
                    },
                    Err(BenchmarkError::EmptyCrawl) => continue,
                    Err(e) => return Err(e),
                }
            }
            match outcome {
                Some(o) => o,
                None => return Err(BenchmarkError::EmptyCrawl),
            }
        },
        Err(error) => return Err(error),
    };
    drop(tmp); // aggregation done; absolute path dies here

    Ok(outcome)
}

/// Capture the static RAM proxy for a strategy by building a mirror
/// downloader through [`DefaultDownloaderFactory`] and reading
/// [`memory_cost`](webfang_core::infrastructure::downloader::Downloader::memory_cost).
/// The engine still does not expose the cost of
/// the downloader it builds, so this parallel construction remains the only
/// seam that observes it without touching core.
fn ram_proxy(strategy: JsStrategy) -> Result<usize> {
    let cookie_bridge = Arc::new(RwLock::new(CookieBridge::default()));
    let tls_emulation = EngineOptions::default().tls_emulation;
    let downloader = DefaultDownloaderFactory
        .build(
            &DownloaderSpec {
                strategy,
                timeout_secs: 30,
                tls_emulation,
                ignore_waf: false,
                user_agent: None,
                // #890: operator headers/cookies are wired on the scrape path
                // only; the benchmark mirror keeps profile defaults.
                custom_headers: Vec::new(),
                accept_language: None,
                initial_cookie_jar: None,
                max_retries: 3,
                backoff_base_ms: 1000,
                backoff_max_ms: 10000,
                obscura_binary: "obscura".to_string(),
            },
            cookie_bridge,
            CancellationToken::new(),
        )
        .map_err(|error| {
            BenchmarkError::Engine(format!("ram-proxy downloader build failed: {error}"))
        })?;
    Ok(downloader.memory_cost())
}
