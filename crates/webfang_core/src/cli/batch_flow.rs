//! CLI batch pipeline — the `--batch` lifecycle end to end.
//!
//! Extracted from `cli/orchestrator.rs` as part of the composition-root
//! decomposition (issue #1619, finding F1). Everything reachable only from
//! `run_batch` lives here: the per-URL crawl, the bounded capture spool,
//! content extraction, the elastic/resume wiring and the batch crawl-config
//! projection.
//!
//! The module owns no new policy. The exit-code decision stays in
//! [`crate::cli::exit_codes`], the canonical severity mapping in
//! [`crate::cli::error`], and the shared gates (`output_vectors_gate`,
//! `export_phase`, the persistence-root resolver) in
//! [`crate::cli::orchestrator`] — so both CLI flows keep one implementation of
//! each rule.

use tracing::{error, info, warn};

use crate::application::batch::{BatchManager, BatchManagerSummary};
use crate::application::crawl_options::{CrawlLimits, CrawlOptions};
use crate::application::crawler::BoundedFileSink;
use crate::cli::discovery_phase::resolve_sitemap_projection;
use crate::cli::elastic::{build_elastic_ingestion, run_elastic_ingestion};
use crate::cli::error::CliExit;
use crate::cli::exit_codes::{batch_exit_code, report_phase};
use crate::cli::orchestrator::{export_phase, output_vectors_gate, resolve_persistence_root};
use crate::domain;
use crate::domain::config::ScraperConfig;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::persistence::StateStorePort;
use crate::CrawlerConfig;

#[cfg(feature = "ai")]
use crate::domain::semantic_cleaner::SemanticCleaner;

/// Run batch processing mode: scrape multiple URLs from stdin or file.
///
/// #1215: `--batch` SCRAPES each input URL (exactly one page per URL) — it
/// never BFS-crawls seeds. The crawl-expansion knobs (`--max-depth`,
/// `--max-pages`, `--sitemap`) do NOT apply here; a non-default value earns
/// a loud warning in [`prepare_batch_manager`] instead of silently changing
/// the run. `--single-page` is honored trivially: single-page is what batch
/// always does.
///
/// The batch pipeline spools every fetched page body through a shared
/// [`BoundedFileSink`] and then runs the full export / elastic / resume
/// pipeline — the same stages `run()` applies to single-page mode, so
/// `--batch` actually writes `.md` + `.jsonl` and honors `--elastic` /
/// `--resume` (#631, #637), with bounded memory (#653).
pub(crate) async fn run_batch(
    opts: CrawlOptions,
    #[cfg(feature = "ai")] ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    vault_ports: crate::application::container::VaultAiPorts,
    _llm_port: Option<std::sync::Arc<dyn crate::domain::llm_port::LlmPort>>,
    cancel: &tokio_util::sync::CancellationToken,
    root_correlation: &domain::CorrelationId,
) -> CliExit {
    // #703/#652: pre-flight gate for `--output-vectors` — same single source of
    // truth as `run()` (see `output_vectors_gate`). First statement here so the
    // exit fires before any crawl, spool, or sink wiring runs.
    if let Some(exit) = output_vectors_gate(&opts) {
        return exit;
    }

    // #796: same `export_format vector` gate as `run()` — covers the batch path
    // so `--batch --export-format vector` without `--clean-ai` also fails fast.
    if let Err(exit) = crate::cli::preflight::check_export_format_vector(&opts) {
        return exit;
    }

    // Resolve the TLS/H2 fingerprint once so the batch crawl engine honors
    // `--h2-profile` (#312). An unknown profile is a config error (exit 78),
    // matching the scrape phase — never silently crawl with a wrong fingerprint.
    let tls_emulation = match resolve_batch_tls_emulation(&opts) {
        Ok(profile) => profile,
        Err(e) => return e,
    };

    let (summary, sink) =
        match run_batch_crawl(&opts, tls_emulation, cancel, root_correlation).await {
            Ok(pair) => pair,
            Err(e) => return e,
        };

    let extracted = extract_batch_content(&sink, &opts).await;
    discard_batch_spool(&sink).await;
    let (results, failures) = match extracted {
        Ok(pair) => pair,
        Err(e) => return e,
    };

    // Issue #1814 (slice B, AC4): share the slice with the blocking-pool
    // export (same rationale as orchestrator::run) — `batch_exit_code` still
    // needs `results.len()` after the export, and an `Arc` move never clones
    // page content.
    let results: std::sync::Arc<[domain::ScrapedContent]> = std::sync::Arc::from(results);

    // Resume mode (#637): construct the state store so `export_phase` can mark
    // each URL as processed — no URL filtering, they were already crawled.
    let state_store = match build_batch_resume_store(&opts) {
        Ok(s) => s,
        Err(e) => return e,
    };

    let elastic_ingestion = match build_elastic_ingestion(&opts, vault_ports).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    if let Err(e) = run_batch_elastic(&elastic_ingestion, &results).await {
        return e;
    }

    // Print extraction failures to stderr (crawl failures are already logged
    // via `log_batch_summary`). Do NOT short-circuit here: always export the
    // pages we did capture so `--batch` writes `.md` + `.jsonl` even on partial
    // failure (#631). The batch path has no robots-blocked counter — its crawl
    // engine reports blocks through `summary` — so pass 0.
    let _ = report_phase(&results, &failures, 0, opts.verbosity);

    #[cfg(feature = "ai")]
    let export_exit =
        export_phase(std::sync::Arc::clone(&results), &opts, state_store.as_deref(), ai_cleaner)
            .await;
    #[cfg(not(feature = "ai"))]
    let export_exit = export_phase(std::sync::Arc::clone(&results), &opts, state_store.as_deref()).await;

    // Final exit code aggregates BOTH crawl-level and extraction-level outcomes
    // with `#537` severity routing: partial success -> 69, all-fail with an
    // internal fatal error -> 3, otherwise 0. Crawl failures were only logged
    // above, so this is the only place the batch's true status surfaces. A
    // failed export (`IoError` 74, `ConfigError` 78, ...) surfaces only when
    // that aggregate is `Success`, mirroring the single-run path's export
    // ordering (orchestrator.rs) — a dropped export exit here used to make a
    // batch with a failed export exit 0 (#1820).

    // Special cell — Cancelled: same precedence as the single-run path —
    // cancellation beats both the classification-based routing and the export
    // exit, and exits 0.
    if let Some(exit) = crate::cli::error::cancelled_exit(cancel.is_cancelled()) {
        return exit;
    }

    let total_failed = summary.failed + failures.len();
    let mut all_errors = summary.errors;
    all_errors.extend(failures);
    let batch_exit = batch_exit_code(results.len(), total_failed, &all_errors);
    if !matches!(batch_exit, CliExit::Success) {
        return batch_exit;
    }
    export_exit
}

/// Scrape every batch URL (one page per URL, #1215), spooling each fetched
/// body to disk, and return the run summary plus the sink holding the spool.
/// Performs the no-URL / no-content guards so `--batch` fails loudly instead
/// of writing nothing (#631).
///
/// The sink is a [`BoundedFileSink`], not an in-memory buffer: a large batch of
/// heavy pages must not grow the resident set without a ceiling (#653).
async fn run_batch_crawl(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    cancel: &tokio_util::sync::CancellationToken,
    root_correlation: &domain::CorrelationId,
) -> Result<(BatchManagerSummary, std::sync::Arc<BoundedFileSink>), CliExit> {
    let sink = std::sync::Arc::new(build_batch_sink(opts).await?);
    let manager =
        prepare_batch_manager(opts, tls_emulation, sink.clone(), root_correlation).await?;

    let summary = manager.process_all_summary_cancellable(cancel).await;
    log_batch_summary(&summary, root_correlation);

    if cancel.is_cancelled() {
        warn!("shutdown requested — exporting the pages captured so far");
        // Stop the spool writer at the next page boundary so a shutdown never
        // waits on a spool that has stopped draining (#1616). `finish` still
        // flushes and joins it, so the pages already persisted are exported.
        sink.cancel();
    }

    flush_batch_sink(&sink).await?;

    Ok((summary, sink))
}

/// Warn when crawl-expansion flags are set on a `--batch` run (#1215).
///
/// Batch scrapes each input URL (one page per URL), so `--max-depth`,
/// `--max-pages`, and `--sitemap` cannot expand anything here. A value that
/// differs from the default means the operator asked for a crawl — staying
/// silent would let them assume one happened — hence `warn!`, naming every
/// inert flag. Defaults are read from [`CrawlLimits::default`] (the same
/// source the CLI defaults mirror) rather than hardcoded, so the comparison
/// cannot rot when defaults move.
fn warn_batch_crawl_flags_ignored(opts: &CrawlOptions) {
    let defaults = CrawlLimits::default();
    let mut inert = Vec::new();
    if opts.crawl.max_depth != defaults.max_depth {
        inert.push(format!("--max-depth {}", opts.crawl.max_depth));
    }
    if opts.crawl.max_pages != defaults.max_pages {
        inert.push(format!("--max-pages {}", opts.crawl.max_pages));
    }
    if opts.crawl.use_sitemap || opts.crawl.sitemap_url.is_some() {
        inert.push("--sitemap".to_string());
    }
    if inert.is_empty() {
        info!("Batch mode scrapes each input URL (one page per URL, no crawling)");
    } else {
        warn!(
            flags = inert.join(", "),
            "--batch scrapes each input URL (one page per URL): crawl-expansion flags are ignored (#1215)"
        );
    }
}

/// Load the batch manager, attach the capture sink, and assert it has work.
///
/// Warns loudly when crawl-expansion flags (`--max-depth`, `--max-pages`,
/// `--sitemap`) differ from their defaults: since #1215 batch scrapes each
/// URL instead of crawling it, those flags are inert here and the operator
/// must hear about it rather than assume a crawl happened.
///
/// `root_correlation` (#1439) travels onto the [`BatchProcessor`] so every
/// per-URL crawl engine of this run shares the one announced identity.
async fn prepare_batch_manager(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    sink: std::sync::Arc<BoundedFileSink>,
    root_correlation: &domain::CorrelationId,
) -> Result<BatchManager, CliExit> {
    warn_batch_crawl_flags_ignored(opts);
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let crawler_config = build_batch_crawler_config(opts, tls_emulation, &budget)?;
    let manager = load_batch_manager(opts, crawler_config, &budget)
        .await?
        .with_content_sink(sink)
        .with_correlation(root_correlation.clone());

    if manager.url_count() == 0 {
        error!("No URLs provided for batch processing");
        return Err(CliExit::UsageError("No URLs provided".into()));
    }

    info!(
        "Starting batch processing: {} URLs, concurrency={}",
        manager.url_count(),
        budget.batch().get()
    );

    Ok(manager)
}

/// Flush the capture spool and fail loudly when the batch produced nothing.
///
/// An empty spool means `--batch` would write zero files while reporting
/// success — the regression #631 fixed.
async fn flush_batch_sink(sink: &BoundedFileSink) -> Result<(), CliExit> {
    let captured = sink.finish().await.map_err(|e| {
        error!(error = %e, "batch content spool flush failed");
        CliExit::IoError(format!("No se pudo volcar el contenido capturado: {e}"))
    })?;

    if captured == 0 {
        error!("Batch captured no page bodies — nothing to export");
        return Err(CliExit::NetworkError("Batch produced no content".into()));
    }

    // The spool could not keep up with the crawl AND the sink's memory ceiling
    // was reached, so some bodies were never spooled (#1616). The crawl itself
    // is sound and the export is still valid, but the run is incomplete and the
    // operator has to be able to tell that from a healthy batch.
    let dropped = sink.dropped();
    if dropped > 0 {
        warn!(
            captured_pages = captured,
            dropped_pages = dropped,
            "batch capture spool saturated — these page bodies were not exported"
        );
    }

    Ok(())
}

/// File name of the batch capture spool, under the run's persistence root.
const BATCH_SPOOL_FILE: &str = ".webfang-batch-capture.jsonl";

/// Spanish disclosure for a spool left behind by a run that did not finish
/// (DF-L1, #1615).
///
/// The spool holds EVERY page body the run fetched, verbatim. Deleting it is
/// best-effort, so a crash — SIGKILL, OOM, panic, power loss — leaves it on
/// disk with nothing scheduled to remove it. That is a data-residue and
/// disk-usage problem, not just hygiene, and the operator cannot see a
/// dotfile they were never told existed. So the leftover is named, sized and
/// located before it goes.
const STALE_SPOOL_MESSAGE: &str =
    "Se encontró un archivo temporal de captura de una ejecución anterior que no finalizó. \
Contiene el texto completo de las páginas descargadas y se ha eliminado. \
Bórrelo manualmente si persiste.";

/// Outcome of the startup sweep for a spool left by a previous run.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StaleSpoolSweep {
    /// Bytes the leftover occupied on disk, for the disclosure.
    reclaimed_bytes: u64,
    /// Whether the file was actually removed. A `false` here means the
    /// disclosure is still owed to the operator, so it is tracked rather than
    /// inferred.
    removed: bool,
}

/// Sweep a batch capture spool left behind by a run that never finished
/// (DF-L1, #1615).
///
/// # Why this runs where it does, and not at every startup
///
/// The obvious stronger version — sweep on every CLI start — is unsafe. The
/// spool path is derived from the persistence root, so two concurrent
/// `webfang` processes sharing an output directory would have the second one
/// delete the first one's in-flight spool, turning a recoverable run into a
/// failed one. Distinguishing them needs a lock or an age heuristic, and both
/// are worse than the problem.
///
/// Sweeping in the batch path, immediately before the same path is truncated,
/// is safe by construction: the file is about to be overwritten regardless, so
/// the sweep can only reclaim space and disclose what it reclaimed. It cannot
/// destroy a live run, and it needs no concurrency reasoning at all.
///
/// The residue therefore persists until the next BATCH run, which is stated
/// here rather than left for an operator to discover. A run that crashes and is
/// never followed by another batch keeps its spool, and the disclosure tells
/// them exactly which file to delete.
///
/// The size is read BEFORE the unlink, because afterwards there is nothing to
/// measure — and a disclosure that cannot say how much it reclaimed is a much
/// weaker prompt to go looking.
async fn sweep_stale_batch_spool(spool_path: &std::path::Path) -> StaleSpoolSweep {
    let metadata = match tokio::fs::metadata(spool_path).await {
        Ok(m) => m,
        // No leftover is the normal case, not a failure worth reporting.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return StaleSpoolSweep {
                reclaimed_bytes: 0,
                removed: true,
            };
        },
        Err(e) => {
            tracing::debug!(
                error = %e,
                spool = %spool_path.display(),
                "stale batch spool could not be inspected"
            );
            return StaleSpoolSweep {
                reclaimed_bytes: 0,
                removed: false,
            };
        },
    };

    let reclaimed_bytes = metadata.len();
    let removed = match tokio::fs::remove_file(spool_path).await {
        Ok(()) => true,
        Err(e) => {
            // The operator still has the file, so the disclosure is still owed.
            // This is a `warn` and not a `debug` for exactly that reason: the
            // old code logged removal failure at debug level, which is how a
            // residue nobody was told about stayed invisible.
            tracing::warn!(
                error = %e,
                spool = %spool_path.display(),
                user_message = STALE_SPOOL_MESSAGE,
                "a batch capture spool from an unfinished run could not be removed"
            );
            false
        },
    };

    if removed {
        // Disclosed in Spanish for the operator; the structured fields stay
        // English so the event is queryable in the JSONL trace.
        tracing::warn!(
            spool = %spool_path.display(),
            reclaimed_bytes,
            user_message = STALE_SPOOL_MESSAGE,
            "removed a batch capture spool left behind by an unfinished run"
        );
    }

    StaleSpoolSweep {
        reclaimed_bytes,
        removed,
    }
}

/// Create the disk-backed capture sink for a batch run.
///
/// The spool lives under the run's persistence root so it shares the run's
/// storage budget (and lands inside the vault when `--quick-save` or an
/// explicit `--vault` redirects the base, #638/#762) and is cleaned up by
/// [`discard_batch_spool`] once extraction is done — or, if the run never
/// finishes, by [`sweep_stale_batch_spool`] on the next batch run (DF-L1).
async fn build_batch_sink(opts: &CrawlOptions) -> Result<BoundedFileSink, CliExit> {
    let spool_path = resolve_persistence_root(opts).join(BATCH_SPOOL_FILE);
    // Before anything writes: reclaim and disclose whatever an unfinished run
    // left. `BoundedFileSink::new` truncates this same path immediately after,
    // so the sweep is a disclosure of residue rather than a deletion decision.
    sweep_stale_batch_spool(&spool_path).await;
    // One buffered page per concurrent crawl, plus headroom, keeps the writer
    // from becoming the bottleneck without unbounding memory. The bound derives
    // from the budget model's Operation.batch tier (task 2.5c).
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let buffer = budget
        .batch()
        .get()
        .saturating_mul(2)
        .max(crate::application::crawler::bounded_sink::DEFAULT_SINK_BUFFER);
    BoundedFileSink::new(spool_path, buffer).await.map_err(|e| {
        error!(error = %e, "batch content spool could not be created");
        CliExit::IoError(format!(
            "No se pudo crear el archivo temporal de captura: {e}"
        ))
    })
}

/// Remove the batch capture spool once its pages have been extracted.
///
/// Best-effort: a leftover spool is noise, not a failure of the run. If THIS
/// removal fails, the residue is not invisible either — [`sweep_stale_batch_spool`]
/// discloses any survivor on the next batch run, which is the same remedy the
/// crash case gets.
async fn discard_batch_spool(sink: &BoundedFileSink) {
    if let Err(e) = tokio::fs::remove_file(sink.spool_path()).await {
        tracing::debug!(
            error = %e,
            spool = %sink.spool_path().display(),
            "batch capture spool could not be removed"
        );
    }
}

/// Build the resume state store for `--resume` (#637) so `export_phase`
/// can mark each already-crawled URL as processed. Returns `None` when
/// `--resume` is off.
///
/// The factory is lazy and infallible, so this always succeeds when resume
/// is on; the `Result` keeps the `CliExit::IoError` route for a future
/// eager factory.
fn build_batch_resume_store(
    opts: &CrawlOptions,
) -> Result<Option<std::sync::Arc<dyn StateStorePort>>, CliExit> {
    if !opts.crawl.resume {
        return Ok(None);
    }
    let state_dir = opts
        .crawl
        .state_dir
        .clone()
        .unwrap_or_else(crate::cli::scrape_flow::resolve_default_state_dir);
    let domain = opts.url.host_str().unwrap_or("batch").to_string();
    Ok(Some(crate::application::container::build_state_store(
        state_dir, &domain,
    )))
}

/// Run the elastic / output-vectors ingestion for the batch pipeline (#636,
/// #637) and release the ingestion handle afterwards.
async fn run_batch_elastic(
    ingestion: &Option<
        std::sync::Arc<
            crate::application::elastic_ingestion::ElasticIngestion<
                crate::domain::repository::DynVectorRepository,
            >,
        >,
    >,
    results: &[domain::ScrapedContent],
) -> Result<(), CliExit> {
    if let Some(ref ingestion) = ingestion {
        run_elastic_ingestion(ingestion, results)
            .await
            .map_err(|e| CliExit::IoError(format!("Falló la ingesta de vectores: {e}")))?;
    }
    Ok(())
}

/// Convert the batch-captured pages into [`ScrapedContent`] and collect
/// per-page extraction failures.
///
/// Pages are streamed one at a time from the sink's spool (#653) — the raw
/// bodies are never all resident at once. Each body goes through the same
/// [`extract_content`] path as single-page mode: Readability → text fallback →
/// binary detection. Pages that fail extraction are logged and reported; the
/// `exit_code` decision is made afterwards by `report_phase`.
///
/// The batch crawl had one fetch per URL, so `CrawlTaskCtx` uses the default
/// asset downloader (`None`) — the same behavior as `--no-images` /
/// `--no-documents`.
///
/// # Errors
///
/// Returns [`CliExit`] when the capture spool cannot be read or decoded.
/// Per-page failures are collected in the `failures` vec instead of aborting
/// the whole batch.
async fn extract_batch_content(
    sink: &BoundedFileSink,
    opts: &CrawlOptions,
) -> Result<
    (
        Vec<domain::ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
    ),
    CliExit,
> {
    let scraper_config = ScraperConfig::default()
        .with_output_dir(resolve_persistence_root(opts))
        .with_selector(opts.crawl.selector.clone())
        .with_ignore_waf(opts.crawl.ignore_waf);

    let root_correlation = domain::CorrelationId::new();
    let mut results = Vec::new();
    let mut failures: Vec<(String, crate::error::ScraperError)> = Vec::new();

    let mut reader = sink.reader().await.map_err(|e| {
        error!(error = %e, "batch capture spool could not be opened");
        CliExit::IoError(format!("No se pudo leer el contenido capturado: {e}"))
    })?;

    while let Some(page) = reader.next_page().await.map_err(|e| {
        error!(error = %e, "batch capture spool could not be decoded");
        CliExit::IoError(format!("No se pudo leer el contenido capturado: {e}"))
    })? {
        let page_correlation = root_correlation.child();
        // Single shared conversion path (P6-2/F-16, #1290): the same helper
        // MCP's session export feeds — identical DTO by construction.
        match crate::application::crawler::content_sink::extract_page_content(
            &page,
            &scraper_config,
            &page_correlation,
        )
        .await
        {
            Ok(content) => results.push(content),
            Err((url, e)) => failures.push((url, e)),
        }
    }

    Ok((results, failures))
}

/// Print the batch completion summary and log each failed URL.
///
/// `root_correlation` is the batch run-root identity: each failed URL emits
/// the shared operational error contract (`log_scrape_error`) carrying it,
/// so trace-file queries join failures with the run (#1604).
fn log_batch_summary(summary: &BatchManagerSummary, root_correlation: &domain::CorrelationId) {
    println!(
        "Batch complete: {}/{} succeeded, {} failed",
        summary.succeeded, summary.total_urls, summary.failed
    );

    for (url, err) in &summary.errors {
        crate::infrastructure::observability::log_scrape_error(
            err,
            url,
            "batch",
            Some(root_correlation),
            "batch URL failed",
        );
    }
}

/// Resolve the TLS/H2 fingerprint for the batch crawl engine.
///
/// An unknown profile is a config error (exit 78), matching the scrape phase —
/// never silently crawl with a wrong fingerprint (#312).
fn resolve_batch_tls_emulation(opts: &CrawlOptions) -> Result<wreq_util::Profile, CliExit> {
    HttpClientConfig::profile_from_name(&opts.network.h2_profile)
        .map_err(|e| CliExit::ConfigError(e.to_string()))
}

/// Build the crawler config for the batch engine, honoring `--h2-profile`.
///
/// `--delay-ms` and the concurrency bound are propagated here (#653): without
/// them the batch engine crawled at full speed with its own default concurrency,
/// making both flags silent no-ops on the `--batch` path.
///
/// #1215: the `max_pages`/`max_depth` values carried here are the CLI crawl
/// budget, but they never expand batch seeds — the processor rebuilds a
/// seed-only config per URL (the batch processor pins depth 0 / 1 page),
/// so every seed is scraped exactly once and `--max-pages 1` can no longer
/// starve seeds.
///
/// The concurrency bound comes from the run's [`BudgetModel`] Operation.crawl
/// tier (task 2.5b).
fn build_batch_crawler_config(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    budget: &crate::domain::budget::BudgetModel,
) -> Result<CrawlerConfig, CliExit> {
    let crawler_config = CrawlerConfig::builder(opts.url.as_url().clone())
        .max_pages(opts.crawl.max_pages)
        .max_depth(opts.crawl.max_depth)
        .include_patterns(opts.crawl.include_patterns.clone())
        .exclude_patterns(opts.crawl.exclude_patterns.clone())
        .ignore_robots(opts.crawl.ignore_robots)
        .sitemap(resolve_sitemap_projection(opts)?)
        .timeout_secs(opts.network.timeout_secs)
        .delay_ms(opts.network.delay_ms)
        // Concurrency bound derives from the run's budget model
        // Operation.crawl tier (task 2.5b), not from the raw CLI flag.
        .concurrency(budget.crawl().nonzero())
        // Bug R2-1: each batch URL is crawled through the Engine via
        // `crawl_site`; without the overrides the Engine drops the explicit
        // --concurrency / --rate-limit-burst and re-derives the auto tiers.
        .budget_overrides(opts.budget_overrides)
        .tls_emulation(tls_emulation)
        .build();
    Ok(crawler_config)
}

/// Load the batch manager from a file or stdin.
///
/// Also reached by the `--batch-file` dry-run preview (`run_dry_run`), which
/// lists the URLs a batch run would scrape without fetching them.
pub(crate) async fn load_batch_manager(
    opts: &CrawlOptions,
    crawler_config: CrawlerConfig,
    budget: &crate::domain::budget::BudgetModel,
) -> Result<BatchManager, CliExit> {
    if let Some(ref path) = opts.batch.batch_file {
        info!("Reading URLs from file: {}", path.display());
        BatchManager::from_file(path, crawler_config, budget.batch().get()).map_err(|e| {
            error!(error = %e, "Failed to read URLs from file");
            CliExit::IoError(format!("Failed to read URLs from file: {e}"))
        })
    } else {
        info!("Reading URLs from stdin");
        load_batch_manager_from_stdin(crawler_config, budget.batch().get()).await
    }
}

/// Read batch URLs from stdin on a blocking thread.
async fn load_batch_manager_from_stdin(
    crawler_config: CrawlerConfig,
    concurrency: usize,
) -> Result<BatchManager, CliExit> {
    // spawn_blocking: stdin read is blocking I/O that must not run on the
    // Tokio async runtime thread pool — it would block other tasks.
    match tokio::task::spawn_blocking(move || BatchManager::from_stdin(crawler_config, concurrency))
        .await
    {
        Ok(result) => result.map_err(|e| {
            error!(error = %e, "Failed to read URLs from stdin");
            CliExit::IoError(format!("Failed to read URLs from stdin: {e}"))
        }),
        Err(join_err) => {
            error!(error = %join_err, "stdin read task panicked");
            Err(CliExit::IoError(format!("Failed to read URLs: {join_err}")))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{build_batch_crawler_config, run_batch};
    use crate::application::crawl_options::CrawlOptions;
    use crate::cli::error::CliExit;
    use crate::CrawlerConfig;

    /// Build the batch config projection for `opts` the way preflight does:
    /// the explicit overrides ride through a freshly built [`BudgetModel`].
    fn projected_config(opts: &CrawlOptions) -> CrawlerConfig {
        let budget = crate::domain::budget::BudgetModel::build(
            opts.budget_overrides,
            &crate::domain::budget::detector::SystemDetector,
        );
        build_batch_crawler_config(opts, wreq_util::Profile::Chrome145, &budget)
            .expect("valid test projection must build")
    }

    #[test]
    fn batch_config_propagates_delay_and_model_concurrency() {
        // Regression for #653: per-URL rate limiting never engaged on the
        // batch path. The concurrency bound now derives from the run's
        // BudgetModel crawl tier; an explicit `--concurrency` value reaches
        // it THROUGH the model (explicit-wins override, design D4).
        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = 750;
        // Explicit flag feeds the model override exactly as preflight does.
        opts.network.concurrency = crate::ConcurrencyConfig::new(2);
        if let Some(explicit) = opts.network.concurrency.get() {
            opts.budget_overrides.crawl =
                crate::domain::budget::tiers::CrawlConcurrency::new(explicit).ok();
        }

        let config = projected_config(&opts);

        assert_eq!(config.delay_ms, 750, "--delay-ms must reach the crawler");
        assert_eq!(
            config.concurrency.get(),
            2,
            "explicit --concurrency must reach the crawler through the model"
        );
    }

    #[test]
    fn batch_config_propagates_budget_overrides() {
        // Bug R2-1: --batch crawls each URL through the crawl Engine via
        // BatchProcessor.process_single_url -> crawl_site; the operator
        // overrides must ride on the base config handed to the batch job.
        let mut opts = CrawlOptions::default();
        opts.budget_overrides.crawl = crate::domain::budget::tiers::CrawlConcurrency::new(4).ok();
        opts.budget_overrides.rate_burst = crate::domain::budget::tiers::BurstPermits::new(13).ok();

        let config = projected_config(&opts);

        assert_eq!(
            config.budget_overrides.crawl.map(|c| c.get()),
            Some(4),
            "explicit --concurrency must reach the batch Engine"
        );
        assert_eq!(
            config.budget_overrides.rate_burst.map(|b| b.get()),
            Some(13),
            "explicit --rate-limit-burst must reach the batch Engine"
        );
    }

    #[test]
    fn batch_config_auto_concurrency_uses_model_tier() {
        // With no explicit flag, the model's auto-derived crawl tier is used.
        let opts = CrawlOptions::default();
        assert!(opts.network.concurrency.is_auto());
        let budget = crate::domain::budget::BudgetModel::for_test_preset();

        let config = build_batch_crawler_config(&opts, wreq_util::Profile::Chrome145, &budget)
            .expect("valid test projection must build");

        assert_eq!(
            config.concurrency.get(),
            budget.crawl().get(),
            "auto mode must use the model's derived Operation.crawl tier"
        );
    }

    #[test]
    fn batch_config_zero_delay_disables_throttling() {
        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = 0;

        let config = build_batch_crawler_config(
            &opts,
            wreq_util::Profile::Chrome145,
            &crate::domain::budget::BudgetModel::for_test_preset(),
        )
        .expect("valid test projection must build");

        assert_eq!(config.delay_ms, 0);
    }

    #[cfg(not(feature = "ai"))]
    #[tokio::test]
    async fn run_batch_returns_config_error_when_output_vectors_without_ai() {
        let mut opts = CrawlOptions::default();
        opts.elastic.output_vectors = Some("vectors.jsonl".to_string());
        let cancel = tokio_util::sync::CancellationToken::new();

        let exit = run_batch(
            opts,
            crate::application::container::VaultAiPorts::default(),
            None,
            &cancel,
            &crate::domain::CorrelationId::new(),
        )
        .await;

        assert!(
            matches!(exit, CliExit::ConfigError(_)),
            "expected CliExit::ConfigError, got {exit:?}"
        );
    }

    #[cfg(feature = "ai")]
    #[tokio::test]
    async fn run_batch_returns_data_error_when_output_vectors_without_clean_ai() {
        let mut opts = CrawlOptions::default();
        opts.elastic.output_vectors = Some("vectors.jsonl".to_string());
        let cancel = tokio_util::sync::CancellationToken::new();

        let exit = run_batch(
            opts,
            None,
            crate::application::container::VaultAiPorts::default(),
            None,
            &cancel,
            &crate::domain::CorrelationId::new(),
        )
        .await;

        assert!(
            matches!(exit, CliExit::DataFormatError(_)),
            "expected CliExit::DataFormatError, got {exit:?}"
        );
    }

    /// Capture-subscriber harness, so the sweep's DISCLOSURE can be read rather
    /// than reviewed (DF-L1, #1615).
    #[derive(Clone)]
    struct SpoolCaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SpoolCaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("capture buffer lock is never poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run an async closure under an in-memory `tracing` subscriber and return
    /// what it logged.
    async fn capture_logs<F, Fut>(f: F) -> (String, ())
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let subscriber = {
            let sink = std::sync::Arc::clone(&buf);
            tracing_subscriber::fmt()
                .with_writer(move || SpoolCaptureWriter(std::sync::Arc::clone(&sink)))
                .with_ansi(false)
                .finish()
        };
        // Thread-local default: the sweep runs on this thread, and installing a
        // global subscriber here would leak into every other test in the crate.
        let _guard = tracing::subscriber::set_default(subscriber);
        f().await;
        let text = String::from_utf8(
            buf.lock()
                .expect("capture buffer lock is never poisoned")
                .clone(),
        )
        .expect("tracing writes UTF-8");
        (text, ())
    }

    /// Run the sweep and return BOTH its verdict and what it logged.
    async fn sweep_capturing(spool: &std::path::Path) -> (super::StaleSpoolSweep, String) {
        let result = std::sync::Arc::new(std::sync::Mutex::new(None));
        let sink = std::sync::Arc::clone(&result);
        let path = spool.to_path_buf();
        let (logs, _) = capture_logs(|| async move {
            let outcome = super::sweep_stale_batch_spool(&path).await;
            *sink.lock().expect("result lock is never poisoned") = Some(outcome);
        })
        .await;
        let outcome = result
            .lock()
            .expect("result lock is never poisoned")
            .take()
            .expect("the closure always records its outcome");
        (outcome, logs)
    }

    /// #1615 DF-L1 — a spool left by a run that never finished is reclaimed AND
    /// disclosed. The residue is the point: it holds every page body the dead
    /// run fetched, so a fix that deleted it silently would still leave the
    /// operator unable to know their disk grew.
    #[tokio::test]
    async fn a_leftover_spool_is_reclaimed_and_disclosed() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let spool = dir.path().join(super::BATCH_SPOOL_FILE);
        // Stand in for the dead run's spool: identifiable page bodies.
        tokio::fs::write(
            &spool,
            "{\"url\":\"https://a.example\",\"body\":\"PAGE\"}\n".repeat(64),
        )
        .await
        .expect("seed a leftover spool");
        let seeded_len = tokio::fs::metadata(&spool).await.expect("size").len();
        assert!(seeded_len > 0, "the seeded leftover must not be empty");

        let (sweep, logs) = sweep_capturing(&spool).await;

        assert!(sweep.removed, "the leftover must be removed");
        assert_eq!(
            sweep.reclaimed_bytes, seeded_len,
            "the disclosure must size the residue it reclaimed"
        );
        assert!(!spool.exists(), "the spool must be gone from disk");
        assert!(
            logs.contains(super::STALE_SPOOL_MESSAGE),
            "the operator must be told their disk held page bodies: {logs}"
        );

        // And the normal case is silent: no leftover must not cry wolf.
        let (_quiet_sweep, quiet_logs) = sweep_capturing(&spool).await;
        assert!(
            !quiet_logs.contains(super::STALE_SPOOL_MESSAGE),
            "a run with no leftover must not claim one: {quiet_logs}"
        );
    }

    /// The Spanish copy is the operator-facing half and is asserted as text,
    /// because a disclosure nobody can read is not a disclosure. The structured
    /// fields stay English so the event is greppable and queryable.
    #[tokio::test]
    async fn the_disclosure_names_the_cause_and_the_size() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let spool = dir.path().join(super::BATCH_SPOOL_FILE);
        tokio::fs::write(&spool, b"residue").await.expect("seed");

        let (_sweep, logs) = sweep_capturing(&spool).await;

        for fragment in [
            "no finalizó",
            "texto completo de las páginas",
            "se ha eliminado",
        ] {
            assert!(
                logs.contains(fragment),
                "the Spanish disclosure must say {fragment:?}: {logs}"
            );
        }
        assert!(
            logs.contains("unfinished run"),
            "the event must be greppable in English: {logs}"
        );
        assert!(
            logs.contains("reclaimed_bytes="),
            "the reclaimed size must be a queryable field: {logs}"
        );
    }

    /// The one case the sweep must NOT absorb silently: a file it could not
    /// remove. The operator still has the residue, so the disclosure is still
    /// owed — and the pre-existing removal path logged that at `debug`, which is
    /// how a residue nobody was told about stayed invisible.
    #[tokio::test]
    #[cfg(unix)]
    async fn a_spool_that_cannot_be_removed_is_still_disclosed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().expect("temp dir");
        let spool = dir.path().join(super::BATCH_SPOOL_FILE);
        tokio::fs::write(&spool, b"residue").await.expect("seed");
        // Make the CONTAINING directory read-only: the file itself stays
        // writable, so the failure is the unlink rather than the inspection.
        let mut perms = std::fs::metadata(dir.path())
            .expect("dir meta")
            .permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(dir.path(), perms).expect("chmod dir");

        let (sweep, logs) = sweep_capturing(&spool).await;

        // Restore before asserting, so a failure does not leave a locked dir.
        let mut perms = std::fs::metadata(dir.path())
            .expect("dir meta")
            .permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(dir.path(), perms).expect("restore dir");

        assert!(
            !sweep.removed,
            "an unremovable spool must be reported as NOT removed"
        );
        assert!(
            spool.exists(),
            "the residue is still on disk, which is why the disclosure is owed"
        );
        assert!(
            logs.contains("could not be removed"),
            "a failed removal must be a warn, not a debug nobody sees: {logs}"
        );
        assert!(
            logs.contains(super::STALE_SPOOL_MESSAGE),
            "the operator must still be told the residue is there: {logs}"
        );
    }
}
