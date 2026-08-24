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

use std::sync::{Arc, RwLock};

use tokio_util::sync::CancellationToken;
use tracing_subscriber::layer::SubscriberExt;
use url::Url;

use crate::aggregate::{self, StrategyMetrics};
use crate::corpus::{self, CorpusManifest};
use crate::error::{BenchmarkError, Result};
use webfang_core::application::crawl_site_with_options;
use webfang_core::application::crawler::{build_fetch_router, EngineOptions};
use webfang_core::domain::value_objects::CorrelationId;
use webfang_core::domain::CrawlerConfig;
use webfang_core::domain::JsStrategy;
use webfang_core::infrastructure::downloader::cookie_bridge::CookieBridge;
use webfang_core::infrastructure::downloader::Downloader;
use webfang_core::infrastructure::observability::FileTraceLayer;

/// Deterministic strategy order for every benchmark invocation.
const STRATEGIES: [JsStrategy; 3] = [JsStrategy::Static, JsStrategy::Hybrid, JsStrategy::Full];

/// One completed strategy run: computed metrics ready for aggregation.
#[derive(Debug)]
pub struct RunOutcome {
    pub strategy: JsStrategy,
    pub metrics: StrategyMetrics,
}

/// Run all strategies sequentially over the corpus and return their metrics.
///
/// Each run serves a fresh corpus (fresh WAF sequence — still deterministic by
/// construction, AC-1.1), writes its own JSONL trace under a private
/// [`tempfile::TempDir`], parses it, computes [`StrategyMetrics`] with the RAM
/// proxy captured from a mirror router build ([`Downloader::memory_cost`] via
/// [`build_fetch_router`], no core change), and drops the TempDir before
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
    let correlation_id = CorrelationId::new();
    tracing::debug!(correlation_id = %correlation_id, "strategy run starting");

    // Fresh corpus per run: fresh WAF sequence, ephemeral port that never
    // reaches compared output.
    let handle = {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        rt.block_on(corpus::serve())?
    };

    let seed = Url::parse(&format!("{}/", handle.base_url))
        .map_err(|source| BenchmarkError::Corpus(format!("invalid corpus base url: {source}")))?;

    let tmp = tempfile::TempDir::new()?;
    let trace_path = tmp.path().join("trace.jsonl");

    // Exact subscriber recipe from benches/tracing_overhead.rs:58-60.
    let layer = FileTraceLayer::new(trace_path.clone())?;
    let subscriber = tracing_subscriber::registry().with(layer);
    let dispatch = tracing::Dispatch::new(subscriber);

    // Small page budget: the corpus has `manifest.pages.len()` distinct paths.
    let config = CrawlerConfig::builder(seed)
        .max_pages(manifest.pages.len())
        .max_depth(1)
        .concurrency(2)
        .delay_ms(0)
        .build();

    let options = EngineOptions {
        js_strategy: strategy,
        ignore_robots: true,
        ..EngineOptions::default()
    };

    let ram_cost_bytes = ram_proxy(strategy)?;

    // Scoped dispatcher on a current_thread runtime: every future polled inside
    // sees this subscriber; nothing global leaks between runs.
    {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let _crawl_result = tracing::dispatcher::with_default(&dispatch, || {
            rt.block_on(crawl_site_with_options(config, options))
        })
        .map_err(|error| BenchmarkError::Engine(error.to_string()))?;
    }
    // `dispatch` dropped above ⇒ FileTraceLayer flushed its buffer to disk
    // before parsing reads the file.

    let records = aggregate::parse_file(&trace_path)?;
    let metrics = aggregate::compute(&records, strategy, ram_cost_bytes)?;
    drop(tmp); // aggregation done; absolute path dies here

    Ok(RunOutcome { strategy, metrics })
}

/// Capture the static RAM proxy for a strategy by building a mirror
/// [`build_fetch_router`] and reading [`Downloader::memory_cost()`]. The engine
/// builds its own router internally, so this parallel construction is the only
/// seam that observes the cost without touching core.
fn ram_proxy(strategy: JsStrategy) -> Result<usize> {
    let cookie_bridge = Arc::new(RwLock::new(CookieBridge::default()));
    let tls_emulation = EngineOptions::default().tls_emulation;
    let router = build_fetch_router(
        &strategy,
        30,
        tls_emulation,
        cookie_bridge,
        false,
        None,
        CancellationToken::new(),
        3,
        1000,
        10000,
        "obscura",
    )
    .map_err(|error| BenchmarkError::Engine(format!("ram-proxy router build failed: {error}")))?;
    Ok(router.memory_cost())
}
