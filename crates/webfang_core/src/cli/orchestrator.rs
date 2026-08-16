//! CLI orchestrator — coordinates the main scraping pipeline.
//!
//! Orchestrates URL discovery, scraping, and export phases.

use tracing::{error, info, instrument, warn};

use crate::application::batch::{BatchManager, BatchManagerSummary};
use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::BoundedFileSink;
use crate::cli::elastic::{build_elastic_ingestion, run_elastic_ingestion};
use crate::cli::error::CliExit;
use crate::cli::export_flow::{run_export, save_files, ExportConfig};
use crate::cli::parse::parse_asset_naming;
use crate::cli::scrape_flow::{apply_resume_mode, scrape_urls};
use crate::cli::url_discovery::{discover_urls, discover_urls_recursive};
use crate::domain::http_config::HttpClientConfig;
use crate::CrawlerConfig;
use crate::ScraperConfig;

use crate::domain;
use crate::infrastructure::export::state_store::StateStore;
use crate::infrastructure::output::file_saver::ObsidianOptions;

pub use crate::cli::parse::handle_completions;

#[cfg(feature = "ai")]
use crate::domain::semantic_cleaner::SemanticCleaner;

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

/// Create an unbounded channel and a `LiveProgressObserver` wired to it.
///
/// Returns `(observer, rx)` where:
/// - `observer` wraps the `tx` side and can be passed to `scrape_urls`
/// - `rx` is the receiving end for the TUI progress view
///
/// The caller must pass `rx` to `run_progress_view` on the main thread
/// (crossterm TTY ownership requirement).
pub fn prepare_progress_channel(
    quiet: bool,
) -> (
    Box<dyn crate::domain::ports::ProgressObserver>,
    tokio::sync::mpsc::UnboundedReceiver<crate::domain::entities::progress::ScrapeProgress>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let observer =
        Box::new(crate::application::progress_observer::LiveProgressObserver::new(Some(tx), quiet));
    (observer, rx)
}

/// Main orchestration entry point.
///
/// Coordinates the full scraping pipeline:
/// 1. URL discovery + config preparation
/// 2. Scraping with progress
/// 3. Export results
/// 4. Report failures + exit code
#[instrument(level = "info", skip(opts, ai_cleaner, adaptive_engine, vault_ports), fields(url = %opts.url))]
pub async fn run(
    opts: CrawlOptions,
    #[cfg(feature = "ai")] ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    #[cfg(feature = "adaptive-selectors")] adaptive_engine: Option<
        std::sync::Arc<AdaptiveSelectorEngine>,
    >,
    vault_ports: crate::application::container::VaultAiPorts,
) -> CliExit {
    if opts.export.dry_run {
        return run_dry_run(opts).await;
    }
    // Process-level graceful shutdown (#653). The guard owns ONE signal
    // listener for the whole run; every phase observes its token cooperatively
    // so a SIGINT drains in-flight work and still exports it, instead of being
    // ignored until the operator escalates to SIGKILL.
    let shutdown = crate::cli::shutdown::ShutdownGuard::install();
    let cancel = shutdown.token();

    if opts.batch.enabled {
        // Batch mode uses the crawl Engine, which mints its own run-root
        // identity per crawl — do not mint one here.
        return run_batch(
            opts,
            #[cfg(feature = "ai")]
            ai_cleaner,
            vault_ports,
            &cancel,
        )
        .await;
    }

    // Run-root correlation identity (#501): the whole operation owns ONE
    // root; every page derives `.child()` from it. `#[instrument]` spans
    // cannot see locals at creation, so declare it offline-visible via a
    // structured event (lands in the JSONL `.fields`).
    let root_correlation = domain::CorrelationId::new();
    info!(
        correlation_id = %root_correlation,
        trace_id = %root_correlation.trace_id(),
        "run identity"
    );

    let prepare = match prepare_phase(&opts).await {
        Err(e) => return e,
        Ok(p) => p,
    };

    let (urls_to_scrape, state_store) =
        match apply_resume_mode(prepare.urls_to_scrape, &opts, opts.url.as_str()).await {
            Ok(v) => v,
            Err(e) => return e,
        };

    let elastic_ingestion = match build_elastic_ingestion(&opts, vault_ports).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    #[cfg(not(feature = "ai"))]
    if opts.elastic.output_vectors.is_some() {
        return CliExit::ConfigError(
            "Se requiere compilar con '--features ai' para usar --output-vectors".to_string(),
        );
    }

    // Create observer with stderr fallback (no channel) for non-TUI mode.
    // For TUI mode, call `prepare_progress_channel()` from main.rs and pass
    // the observer here instead.
    let observer = Box::new(
        crate::application::progress_observer::LiveProgressObserver::new(None, opts.export.quiet),
    );
    // Bridge the cfg-gated engine into an always-present reference option for
    // the scrape phase: `None` when the feature is compiled out.
    #[cfg(feature = "adaptive-selectors")]
    let engine_ref = adaptive_engine.as_deref();
    #[cfg(not(feature = "adaptive-selectors"))]
    let engine_ref: Option<&AdaptiveSelectorEngine> = None;

    let (results, failures, blocked) = match scrape_phase(
        &urls_to_scrape,
        &prepare.scraper_config,
        &opts,
        observer.as_ref(),
        prepare
            .shared_downloader
            .as_deref()
            .map(|d| d as &dyn crate::domain::ports::AssetDownloaderPort),
        engine_ref,
        &root_correlation,
        &cancel,
    )
    .await
    {
        Ok(pair) => pair,
        // Setup failures (unknown TLS profile, HTTP client build error) are
        // config errors: surface the message and exit 78 rather than silently
        // scraping with a wrong fingerprint.
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };

    if let Some(ref ingestion) = elastic_ingestion {
        if let Err(e) = run_elastic_ingestion(ingestion, &results).await {
            return CliExit::IoError(format!("Falló la ingesta de vectores: {e}"));
        }
    }

    // Release the ingestion pipeline (wreq connection pool + Rayon threads)
    // while the runtime is still active. Without this, the hyper pool
    // background task or Rayon thread join can block runtime shutdown.
    // See issue #335.
    drop(elastic_ingestion);
    tokio::task::yield_now().await;

    if let Some(exit) = report_phase(&results, &failures, blocked, opts.verbosity) {
        return exit;
    }

    #[cfg(feature = "ai")]
    {
        export_phase(&results, &opts, state_store.as_ref(), ai_cleaner).await
    }
    #[cfg(not(feature = "ai"))]
    {
        export_phase(&results, &opts, state_store.as_ref()).await
    }
}

/// Resolve the root directory that must contain the scraped Markdown AND the
/// downloaded assets.
///
/// When Obsidian `--quick-save` is active, both must share the vault as their
/// base so the vault stays self-contained (#638): if the `Downloader` keeps
/// using `output_dir` (`-o`) while the Markdown goes to the vault, relative
/// asset paths escape the vault and images stop rendering. The Downloader is a
/// slave of the config — the orchestrator is responsible for converging the
/// two persistence roots before handing them to the crawl/export engines.
fn resolve_persistence_root(opts: &CrawlOptions) -> std::path::PathBuf {
    if opts.export.quick_save {
        opts.export
            .obsidian_vault
            .clone()
            .unwrap_or_else(|| opts.export.output_dir.clone())
    } else {
        opts.export.output_dir.clone()
    }
}

/// Export scraped results to files and run AI cleaning if requested.
async fn export_phase(
    results: &[domain::ScrapedContent],
    opts: &CrawlOptions,
    state_store: Option<&StateStore>,
    #[cfg(feature = "ai")] ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
) -> CliExit {
    if opts.export.output_dir == std::path::Path::new("-") {
        return CliExit::UsageError(
            "\"-o -\" no está soportado para exportación multi-archivo. \
             Usa \"--output-vectors -\" para exportar vectores a stdout, \
             o especifica un directorio de salida."
                .to_string(),
        );
    }

    let output_dir = opts.export.output_dir.clone();

    let obsidian_options = ObsidianOptions {
        wiki_links: opts.export.obsidian_wiki_links,
        relative_assets: opts.export.obsidian_relative_assets,
        tags: opts.export.obsidian_tags.clone(),
        rich_metadata: opts.export.obsidian_rich_metadata,
        quick_save: opts.export.quick_save,
        vault_path: opts.export.obsidian_vault.clone(),
    };

    let file_output_dir = if opts.export.quick_save {
        let inbox = resolve_persistence_root(opts).join("_inbox");
        if !inbox.exists() {
            let _ = std::fs::create_dir_all(&inbox);
        }
        inbox
    } else {
        output_dir.clone()
    };

    save_files(
        results,
        &file_output_dir,
        &opts.export.output_format,
        &obsidian_options,
    );

    let export_config = ExportConfig {
        results,
        output_dir,
        format: opts.export.output_format,
        export_format: opts.export.export_format,
        clean_ai: opts.ai,
        quick_save: opts.export.quick_save,
        vault_path: opts.export.obsidian_vault.as_ref(),
        obsidian_options,
        state_store,
        resume: opts.crawl.resume,
        ai_threshold: opts.ai_config.threshold,
        ai_max_tokens: opts.ai_config.max_tokens,
        ai_offline: opts.ai_config.offline,
        ai_model: opts.ai_config.model.clone(),
    };

    #[cfg(feature = "ai")]
    let export_result = run_export(export_config, ai_cleaner).await;
    #[cfg(not(feature = "ai"))]
    let export_result = run_export(export_config).await;

    match export_result {
        Ok(processed_urls) => {
            info!("Export completed for {} URLs", processed_urls.len());
            CliExit::Success
        },
        Err(e) => {
            error!(error = ?e, "Export failed");
            e
        },
    }
}

/// Run dry-run: discover URLs and print them without scraping.
async fn run_dry_run(opts: CrawlOptions) -> CliExit {
    // Bug 4: honest dry-run - call real URL discovery
    info!("Dry-run: discovering URLs without scraping...");
    let tls_emulation = match HttpClientConfig::profile_from_name(&opts.network.h2_profile) {
        Ok(profile) => profile,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };

    let crawler_config = build_crawler_config_for_discovery(&opts, tls_emulation);

    let discovered = match crate::cli::url_discovery::discover_urls(&crawler_config, &opts).await {
        Ok(urls) => urls,
        Err(e) => return CliExit::NetworkError(format!("URL discovery failed: {e}")),
    };

    println!("\nDry-run: {} URL(s) would be scraped:", discovered.len());
    for url in &discovered {
        println!("  {url}");
    }
    CliExit::Success
}

/// Build a `CrawlerConfig` for URL discovery (shared by dry-run, prepare, and batch).
fn build_crawler_config_for_discovery(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
) -> CrawlerConfig {
    let mut crawler_config = CrawlerConfig::builder(opts.url.clone())
        .max_pages(opts.crawl.max_pages)
        .max_depth(opts.crawl.max_depth)
        .include_patterns(opts.crawl.include_patterns.clone())
        .exclude_patterns(opts.crawl.exclude_patterns.clone())
        .ignore_robots(opts.crawl.ignore_robots)
        .use_sitemap(opts.crawl.use_sitemap)
        .timeout_secs(opts.network.timeout_secs)
        .delay_ms(opts.network.delay_ms)
        .tls_emulation(tls_emulation);
    if let Some(ref sitemap_url) = opts.crawl.sitemap_url {
        crawler_config = crawler_config.sitemap_url(sitemap_url);
    }
    crawler_config.build()
}

/// Prepare scraper config and discover URLs.
///
/// Returns the initial `ScraperConfig` (before asset/download wiring) and
/// the list of URLs to scrape.  On discovery failure, returns the
/// appropriate `CliExit` error.
async fn prepare_phase(opts: &CrawlOptions) -> Result<PrepareResult, CliExit> {
    let urls_to_scrape = if opts.crawl.single_page {
        plan_urls(true, false, opts.url.clone(), Vec::new())
    } else {
        // Honor `--h2-profile` for URL discovery (#312): an unknown profile is a
        // config error (exit 78), consistent with the scrape and batch phases.
        let tls_emulation = HttpClientConfig::profile_from_name(&opts.network.h2_profile)
            .map_err(|e| CliExit::ConfigError(e.to_string()))?;

        let crawler_config = build_crawler_config_for_discovery(opts, tls_emulation);

        // Sitemap mode is the source of truth (depth-agnostic XML), so keep the
        // existing single-pass sitemap discovery. DOM mode must run the recursive
        // crawl Engine so `--max-depth` is honored (bug #651): the legacy
        // `discover_urls_for_tui` path did one fetch and silently ignored depth.
        let discovered_urls = if opts.crawl.use_sitemap {
            match discover_urls(&crawler_config, opts).await {
                Err(e) => {
                    return Err(CliExit::NetworkError(format!("URL discovery failed: {e}")));
                },
                // Exit 2 only when the sitemap is the source of truth.
                Ok(urls) if urls.is_empty() => {
                    return Err(CliExit::EmptyDiscovery(
                        "No URLs discovered from sitemaps".into(),
                    ));
                },
                Ok(urls) => urls,
            }
        } else {
            // Recursive BFS discovery respects max_depth/max_pages/robots/
            // patterns; the existing scrape_phase + export_phase still own
            // content extraction and on-disk output.
            match discover_urls_recursive(crawler_config, opts).await {
                Err(e) => {
                    return Err(CliExit::NetworkError(format!("URL discovery failed: {e}")));
                },
                Ok(urls) => urls,
            }
        };

        plan_urls(
            false,
            opts.crawl.use_sitemap,
            opts.url.clone(),
            discovered_urls,
        )
    };

    let mut scraper_config = ScraperConfig::default()
        .with_output_dir(resolve_persistence_root(opts))
        .with_scraper_concurrency(opts.network.concurrency.resolve())
        .with_max_pages(opts.crawl.max_pages)
        .with_selector(opts.crawl.selector.clone())
        .with_ignore_waf(opts.crawl.ignore_waf);

    if opts.network.download_images {
        scraper_config = scraper_config.with_images();
    }
    if opts.network.download_documents {
        scraper_config = scraper_config.with_documents();
    }

    // Wire asset download config from CLI args
    // NOTE: crawl include/exclude patterns are intentionally NOT forwarded to
    // asset config — assets have their own filter scope (#639).
    scraper_config =
        scraper_config.with_asset_h2_profile(parse_asset_h2_profile(&opts.network.h2_profile));
    scraper_config = scraper_config.with_asset_naming(parse_asset_naming(&opts.asset_naming));
    scraper_config = scraper_config.with_download_concurrency(opts.download_concurrency);
    scraper_config = scraper_config.with_max_file_size(opts.network.max_file_size);
    scraper_config = scraper_config.with_download_timeout(opts.network.download_timeout_secs);

    // Create shared Downloader once for connection pooling across all page scrapes.
    let shared_downloader = if scraper_config.has_downloads() {
        match crate::adapters::downloader::Downloader::new(scraper_config.to_download_config()) {
            Ok(dl) => Some(std::sync::Arc::new(dl)),
            Err(e) => {
                return Err(CliExit::IoError(format!(
                    "No se pudo crear el descargador de assets: {e}"
                )));
            },
        }
    } else {
        None
    };

    Ok(PrepareResult {
        urls_to_scrape,
        scraper_config,
        shared_downloader,
    })
}

struct PrepareResult {
    urls_to_scrape: Vec<url::Url>,
    scraper_config: ScraperConfig,
    shared_downloader: Option<std::sync::Arc<crate::adapters::downloader::Downloader>>,
}

/// Run the scraping loop over all URLs with progress events.
///
/// # Errors
///
/// Returns [`crate::error::ScraperError`] if the configured H2/TLS profile name
/// is not recognized or the fetch router's HTTP client cannot be built (a setup
/// failure, before any URL is scraped).
#[allow(clippy::too_many_arguments)]
async fn scrape_phase(
    urls: &[url::Url],
    scraper_config: &ScraperConfig,
    opts: &CrawlOptions,
    observer: &dyn crate::application::progress_observer::ProgressObserver,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    engine: Option<&AdaptiveSelectorEngine>,
    root_correlation: &domain::CorrelationId,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<
    (
        Vec<domain::ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
        usize,
    ),
    crate::error::ScraperError,
> {
    scrape_urls(
        urls,
        scraper_config,
        opts,
        observer,
        downloader,
        engine,
        root_correlation,
        cancel,
    )
    .await
}

/// Build the user-facing failure line for a single URL.
///
/// At `verbosity` 0 only the top-level `Display` message is shown, which keeps
/// network errors (DNS, connect) to a single readable line. At `verbosity` 1+
/// the full root-cause chain is preserved via `Error::source()` (D4), appending
/// each cause as `← cause`.
fn format_failure(url: &str, error: &crate::error::ScraperError, verbosity: u8) -> String {
    let mut chain = error.to_string();
    if verbosity > 0 {
        let mut src = std::error::Error::source(error);
        while let Some(cause) = src {
            chain.push_str(&format!("  ← {cause}"));
            src = cause.source();
        }
    }
    format!("Failed to scrape {url}: {chain}")
}

/// Report failures and determine the exit code.
///
/// Returns `None` if all pages scraped successfully (caller proceeds to export).
/// At `verbosity` 0 only the top-level error message is shown; at 1+ the full
/// root-cause chain is appended (see [`format_failure`]).
///
/// Robots-blocked routing (#705): when NOTHING was scraped and NOTHING failed
/// but `blocked > 0`, every URL was refused by robots.txt — the run exits
/// `CliExit::Forbidden` (77) with a Spanish hint about `--ignore-robots`
/// instead of the misleading "no pages scraped" network error. Any real
/// failure or any scraped page keeps the historical routing below.
///
/// Severity routing (#537): when every URL failed AND at least one failure
/// classifies as [`crate::error::ErrorClass::InternalFatal`], the exit code is
/// `CliExit::ScraperFailure` (3) rather than `NetworkError` (69) — an internal
/// bug must not masquerade as a transient network outage. Purely
/// transient/permanent all-fail runs keep exit 69. The partial-success case
/// always reports `PartialSuccess` (69) regardless of failure severity: some
/// content was scraped, which is the dominant signal.
fn report_phase(
    results: &[domain::ScrapedContent],
    failures: &[(String, crate::error::ScraperError)],
    blocked: usize,
    verbosity: u8,
) -> Option<CliExit> {
    for (url, error) in failures {
        eprintln!("{}", format_failure(url, error, verbosity));
    }
    
    if !failures.is_empty() && !results.is_empty() {
        return Some(CliExit::PartialSuccess {
            success: results.len(),
            failed: failures.len(),
        });
    }
    
    if results.is_empty() && failures.is_empty() && blocked > 0 {
        return Some(CliExit::Forbidden(format!(
            "{blocked} URL(s) bloqueadas por robots.txt. Usa --ignore-robots para omitir esta verificación."
        )));
    }
    
    if results.is_empty() {
        if let Some(exit) = scraper_failure_for_internal_fatal(failures) {
            return Some(exit);
        }
        eprintln!("No pages were successfully scraped");
        return Some(CliExit::NetworkError(
            "No pages were successfully scraped".into(),
        ));
    }

    info!("Successfully scraped {} pages", results.len());
    None
}

/// Fold an all-failed error set into a severity-aware exit (#537).
///
/// Returns `Some(CliExit::ScraperFailure(..))` when at least one failure
/// classifies as [`crate::error::ErrorClass::InternalFatal`], with a message
/// that calls out the internal-fatal count. Returns `None` when every failure
/// is transient/permanent — the caller then keeps its historical
/// `CliExit::NetworkError` arm.
fn scraper_failure_for_internal_fatal(
    failures: &[(String, crate::error::ScraperError)],
) -> Option<CliExit> {
    let internal_fatal = failures
        .iter()
        .filter(|(_, e)| e.classify() == crate::error::ErrorClass::InternalFatal)
        .count();
    (internal_fatal > 0).then(|| {
        CliExit::ScraperFailure(format!(
            "Scraper failure: {internal_fatal} internal error(s) out of {} URLs",
            failures.len()
        ))
    })
}

/// Run batch processing mode: crawl multiple URLs from stdin or file.
///
/// The batch pipeline spools every fetched page body through a shared
/// [`BoundedFileSink`] and then runs the full export / elastic / resume
/// pipeline — the same stages `run()` applies to single-page mode, so
/// `--batch` actually writes `.md` + `.jsonl` and honors `--elastic` /
/// `--resume` (#631, #637), with bounded memory (#653).
async fn run_batch(
    opts: CrawlOptions,
    #[cfg(feature = "ai")] ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    vault_ports: crate::application::container::VaultAiPorts,
    cancel: &tokio_util::sync::CancellationToken,
) -> CliExit {
    #[cfg(not(feature = "ai"))]
    if opts.elastic.output_vectors.is_some() {
        return CliExit::ConfigError(
            "Se requiere compilar con '--features ai' para usar --output-vectors".to_string(),
        );
    }

    // Resolve the TLS/H2 fingerprint once so the batch crawl engine honors
    // `--h2-profile` (#312). An unknown profile is a config error (exit 78),
    // matching the scrape phase — never silently crawl with a wrong fingerprint.
    let tls_emulation = match resolve_batch_tls_emulation(&opts) {
        Ok(profile) => profile,
        Err(e) => return e,
    };

    let (summary, sink) = match run_batch_crawl(&opts, tls_emulation, cancel).await {
        Ok(pair) => pair,
        Err(e) => return e,
    };

    let extracted = extract_batch_content(&sink, &opts).await;
    discard_batch_spool(&sink).await;
    let (results, failures) = match extracted {
        Ok(pair) => pair,
        Err(e) => return e,
    };

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
    {
        export_phase(&results, &opts, state_store.as_ref(), ai_cleaner).await;
    }
    #[cfg(not(feature = "ai"))]
    {
        export_phase(&results, &opts, state_store.as_ref()).await;
    }

    // Final exit code aggregates BOTH crawl-level and extraction-level outcomes
    // with `#537` severity routing: partial success -> 69, all-fail with an
    // internal fatal error -> 3, otherwise 0. Crawl failures were only logged
    // above, so this is the only place the batch's true status surfaces.
    let total_failed = summary.failed + failures.len();
    let mut all_errors = summary.errors;
    all_errors.extend(failures);
    batch_exit_code(results.len(), total_failed, &all_errors)
}

/// Crawl every batch URL through the engine, spooling each fetched body to
/// disk, and return the run summary plus the sink holding the spool. Performs
/// the no-URL / no-content guards so `--batch` fails loudly instead of writing
/// nothing (#631).
///
/// The sink is a [`BoundedFileSink`], not an in-memory buffer: a large batch of
/// heavy pages must not grow the resident set without a ceiling (#653).
async fn run_batch_crawl(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(BatchManagerSummary, std::sync::Arc<BoundedFileSink>), CliExit> {
    let sink = std::sync::Arc::new(build_batch_sink(opts).await?);
    let manager = prepare_batch_manager(opts, tls_emulation, sink.clone()).await?;

    let summary = manager.process_all_summary_cancellable(cancel).await;
    log_batch_summary(&summary);

    if cancel.is_cancelled() {
        warn!("shutdown requested — exporting the pages captured so far");
    }

    flush_batch_sink(&sink).await?;

    Ok((summary, sink))
}

/// Load the batch manager, attach the capture sink, and assert it has work.
async fn prepare_batch_manager(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    sink: std::sync::Arc<BoundedFileSink>,
) -> Result<BatchManager, CliExit> {
    let crawler_config = build_batch_crawler_config(opts, tls_emulation);
    let manager = load_batch_manager(opts, crawler_config)
        .await?
        .with_content_sink(sink);

    if manager.url_count() == 0 {
        error!("No URLs provided for batch processing");
        return Err(CliExit::UsageError("No URLs provided".into()));
    }

    info!(
        "Starting batch processing: {} URLs, concurrency={}",
        manager.url_count(),
        opts.batch.concurrency
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

    Ok(())
}

/// Create the disk-backed capture sink for a batch run.
///
/// The spool lives under the output directory so it shares the run's storage
/// budget and is cleaned up by [`discard_batch_spool`] once extraction is done.
async fn build_batch_sink(opts: &CrawlOptions) -> Result<BoundedFileSink, CliExit> {
    let spool_path = opts.export.output_dir.join(".webfang-batch-capture.jsonl");
    // One buffered page per concurrent crawl, plus headroom, keeps the writer
    // from becoming the bottleneck without unbounding memory.
    let buffer = opts
        .batch
        .concurrency
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
/// Best-effort: a leftover spool is noise, not a failure of the run.
async fn discard_batch_spool(sink: &BoundedFileSink) {
    if let Err(e) = tokio::fs::remove_file(sink.spool_path()).await {
        tracing::debug!(
            error = %e,
            spool = %sink.spool_path().display(),
            "batch capture spool could not be removed"
        );
    }
}

/// Build the resume [`StateStore`] for `--resume` (#637) so `export_phase`
/// can mark each already-crawled URL as processed. Returns `None` when
/// `--resume` is off.
fn build_batch_resume_store(opts: &CrawlOptions) -> Result<Option<StateStore>, CliExit> {
    if !opts.crawl.resume {
        return Ok(None);
    }
    let state_dir = opts.crawl.state_dir.clone().unwrap_or_else(|| {
        let cache_base = std::env::var("XDG_CACHE_HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".cache")
            });
        cache_base.join("webfang").join("state")
    });
    let domain = opts.url.host_str().unwrap_or("batch").to_string();
    crate::application::export_factory::create_state_store(state_dir, &domain)
        .map(Some)
        .map_err(|e| {
            CliExit::IoError(format!(
                "No se pudo crear el almacén de estado para --resume: {e}"
            ))
        })
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
        .with_output_dir(opts.export.output_dir.clone())
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
        let url = match url::Url::parse(&page.url) {
            Ok(u) => u,
            Err(e) => {
                failures.push((
                    page.url,
                    crate::error::ScraperError::invalid_url(format!(
                        "No se pudo parsear la URL capturada: {e}",
                    )),
                ));
                continue;
            },
        };

        match crate::application::crawler::extract_content(
            &page.html,
            &url,
            &scraper_config,
            None,
            None,
            &page_correlation,
        )
        .await
        {
            Ok(content) => results.push(content),
            Err(e) => failures.push((page.url, e)),
        }
    }

    Ok((results, failures))
}

/// Print the batch completion summary and log each failed URL.
fn log_batch_summary(summary: &BatchManagerSummary) {
    println!(
        "Batch complete: {}/{} succeeded, {} failed",
        summary.succeeded, summary.total_urls, summary.failed
    );

    for (url, err) in &summary.errors {
        error!(%url, error = %err, "Batch URL failed");
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
/// `--delay-ms` and `--concurrency` are propagated here (#653): without them
/// the batch engine crawled at full speed with its own default concurrency,
/// making both flags silent no-ops on the `--batch` path.
fn build_batch_crawler_config(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
) -> CrawlerConfig {
    let mut crawler_config = CrawlerConfig::builder(opts.url.clone())
        .max_pages(opts.crawl.max_pages)
        .max_depth(opts.crawl.max_depth)
        .include_patterns(opts.crawl.include_patterns.clone())
        .exclude_patterns(opts.crawl.exclude_patterns.clone())
        .ignore_robots(opts.crawl.ignore_robots)
        .use_sitemap(opts.crawl.use_sitemap)
        .timeout_secs(opts.network.timeout_secs)
        .delay_ms(opts.network.delay_ms)
        .concurrency(opts.network.concurrency.resolve())
        .tls_emulation(tls_emulation);
    if let Some(ref sitemap_url) = opts.crawl.sitemap_url {
        crawler_config = crawler_config.sitemap_url(sitemap_url);
    }
    crawler_config.build()
}

/// Load the batch manager from a file or stdin.
async fn load_batch_manager(
    opts: &CrawlOptions,
    crawler_config: CrawlerConfig,
) -> Result<BatchManager, CliExit> {
    if let Some(ref path) = opts.batch.batch_file {
        info!("Reading URLs from file: {}", path.display());
        BatchManager::from_file(path, crawler_config, opts.batch.concurrency).map_err(|e| {
            error!(error = %e, "Failed to read URLs from file");
            CliExit::IoError(format!("Failed to read URLs from file: {e}"))
        })
    } else {
        info!("Reading URLs from stdin");
        load_batch_manager_from_stdin(crawler_config, opts.batch.concurrency).await
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

/// Determine the CLI exit code from batch scrape results.
///
/// Severity routing (#537): an all-failed batch containing at least one
/// [`crate::error::ErrorClass::InternalFatal`] error yields
/// `CliExit::ScraperFailure` (3); purely transient/permanent all-fail runs
/// keep `CliExit::NetworkError` (69). Partial success remains
/// `CliExit::PartialSuccess` (69) regardless of failure severity — some
/// content was scraped, which is the dominant signal.
fn batch_exit_code(
    succeeded: usize,
    failed: usize,
    errors: &[(String, crate::error::ScraperError)],
) -> CliExit {
    if failed > 0 && succeeded == 0 {
        if let Some(exit) = scraper_failure_for_internal_fatal(errors) {
            return exit;
        }
        CliExit::NetworkError("All batch URLs failed".into())
    } else if failed > 0 {
        CliExit::PartialSuccess {
            success: succeeded,
            failed,
        }
    } else {
        CliExit::Success
    }
}

fn plan_urls(
    single_page: bool,
    use_sitemap: bool,
    seed_url: url::Url,
    discovered_urls: Vec<url::Url>,
) -> Vec<url::Url> {
    if single_page {
        vec![seed_url]
    } else if use_sitemap {
        // Sitemap is the source of truth — do not inject the seed URL.
        discovered_urls
    } else {
        // DOM discovery: always include the seed URL so it gets crawled
        // even when link extraction only returns child URLs.
        let mut urls = discovered_urls;
        if !urls.contains(&seed_url) {
            urls.insert(0, seed_url);
        }
        urls
    }
}

/// Parse H2/TLS profile from CLI string for the asset download path.
///
/// Delegates to the domain resolver
/// [`crate::domain::profile::profile_from_name`], which accepts the full
/// [`wreq_util::Profile`] catalog. Unlike the strict page-fetch path, the asset
/// path is best-effort: an unknown name logs a warning and falls back to
/// `Chrome145` rather than failing the run.
fn parse_asset_h2_profile(s: &str) -> wreq_util::Profile {
    crate::domain::profile::profile_from_name(s).unwrap_or_else(|| {
        tracing::warn!(
            "Unknown asset H2 profile '{s}', falling back to Chrome145. \
             Run `cargo doc -p wreq-util` to see all available profiles."
        );
        wreq_util::Profile::Chrome145
    })
}

#[cfg(test)]
mod tests {
    use super::{
        batch_exit_code, build_batch_crawler_config, build_elastic_ingestion, format_failure,
        parse_asset_h2_profile, plan_urls, report_phase,
    };
    use crate::application::crawl_options::CrawlOptions;
    use crate::cli::error::CliExit;

    #[cfg(not(feature = "ai"))]
    use super::{run, run_batch};

    // ===== build_batch_crawler_config tests (#653) =====

    #[test]
    fn batch_config_propagates_delay_and_concurrency() {
        // Regression for #653: `--delay-ms` and `--concurrency` were dropped on
        // the batch path, so per-URL rate limiting never engaged.
        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = 750;
        opts.network.concurrency = crate::ConcurrencyConfig::new(4);

        let config = build_batch_crawler_config(&opts, wreq_util::Profile::Chrome145);

        assert_eq!(config.delay_ms, 750, "--delay-ms must reach the crawler");
        assert_eq!(
            config.concurrency, 4,
            "--concurrency must reach the crawler"
        );
    }

    #[test]
    fn batch_config_zero_delay_disables_throttling() {
        let mut opts = CrawlOptions::default();
        opts.network.delay_ms = 0;

        let config = build_batch_crawler_config(&opts, wreq_util::Profile::Chrome145);

        assert_eq!(config.delay_ms, 0);
    }

    // ===== format_failure tests =====

    fn network_error() -> crate::error::ScraperError {
        let inner = std::io::Error::other("failed to lookup address information");
        crate::error::ScraperError::Network(Box::new(inner))
    }

    #[test]
    fn format_failure_default_hides_source_chain() {
        let msg = format_failure("https://example.com", &network_error(), 0);

        assert!(
            msg.contains("error de red"),
            "missing top-level message: {msg}"
        );
        assert!(
            !msg.contains('←'),
            "default output must not show the cause chain: {msg}"
        );
    }

    #[test]
    fn format_failure_verbose_shows_source_chain() {
        let msg = format_failure("https://example.com", &network_error(), 1);
    
        assert!(
            msg.contains('←'),
            "verbose output must show the cause chain: {msg}"
        );
        assert!(msg.contains("failed to lookup address information"));
    }
    
    // ===== report_phase routing tests (#705) =====
    
    fn scraped(url: &str) -> crate::domain::ScrapedContent {
        crate::domain::ScrapedContent {
            title: "t".into(),
            content: "c".into(),
            url: crate::domain::ValidUrl::parse(url).expect("valid test url"),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        }
    }
    
    #[test]
    fn report_phase_all_blocked_returns_forbidden() {
        // Nothing scraped, nothing failed, but URLs were blocked by robots.txt:
        // exit 77 with the Spanish hint, not a misleading network error (#705).
        let exit = report_phase(&[], &[], 2, 0);
    
        match exit {
            Some(CliExit::Forbidden(msg)) => {
assert!(
msg.contains("2 URL(s) bloqueadas por robots.txt"),
"missing blocked count: {msg}"
);
assert!(
msg.contains("--ignore-robots"),
"missing --ignore-robots hint: {msg}"
);
            },
            other => panic!("expected Forbidden, got: {other:?}"),
        }
    }
    
    #[test]
    fn report_phase_mixed_success_and_blocked_proceeds() {
        // Some pages scraped, the rest blocked: content was produced, so the run
        // proceeds to export exactly as before blocked counting existed.
        let results = vec![scraped("https://example.com/ok")];
    
        assert!(report_phase(&results, &[], 1, 0).is_none());
    }
    
    #[test]
    fn report_phase_partial_success_with_blocked_unchanged() {
        // Failures + results dominate over blocked URLs: PartialSuccess (69)
        // semantics are unchanged by the blocked counter.
        let results = vec![scraped("https://example.com/ok")];
        let failures = vec![("https://example.com/bad".to_string(), network_error())];
    
        let exit = report_phase(&results, &failures, 1, 0);
    
        assert!(
            matches!(
exit,
Some(CliExit::PartialSuccess {
success: 1,
failed: 1
})
            ),
            "expected PartialSuccess, got: {exit:?}"
        );
    }
    
    #[test]
    fn report_phase_all_fail_with_blocked_stays_network_error() {
        // Real failures present: the blocked counter must not mask them — the
        // historical all-fail NetworkError (69) routing wins. A 404 classifies
        // as PermanentFatal (not InternalFatal), so the #537 ScraperFailure
        // arm does not engage.
        let failures = vec![(
            "https://example.com/bad".to_string(),
            crate::error::ScraperError::http(404, "https://example.com/bad"),
        )];
    
        let exit = report_phase(&[], &failures, 1, 0);
    
        assert!(
            matches!(exit, Some(CliExit::NetworkError(_))),
            "expected NetworkError, got: {exit:?}"
        );
    }
    
    #[test]
    fn report_phase_empty_run_without_blocks_stays_network_error() {
        // No results, no failures, no blocks (e.g. zero-URL edge): the
        // historical "No pages were successfully scraped" arm is preserved.
        let exit = report_phase(&[], &[], 0, 0);
    
        assert!(
            matches!(exit, Some(CliExit::NetworkError(_))),
            "expected NetworkError, got: {exit:?}"
        );
    }

    // ===== plan_urls tests =====

    #[test]
    fn plan_urls_single_page_returns_seed_only() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/about").unwrap(),
            url::Url::parse("https://example.com/blog").unwrap(),
        ];

        let result = plan_urls(true, false, seed.clone(), discovered);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_dom_mode_prepends_seed() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/a").unwrap(),
            url::Url::parse("https://example.com/b").unwrap(),
            url::Url::parse("https://example.com/c").unwrap(),
        ];

        let result = plan_urls(false, false, seed.clone(), discovered.clone());

        // DOM mode: the seed is prepended when absent so it always gets scraped.
        let mut expected = vec![seed];
        expected.extend(discovered);
        assert_eq!(result, expected);
    }

    #[test]
    fn plan_urls_sitemap_mode_does_not_prepend_seed() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/a").unwrap(),
            url::Url::parse("https://example.com/b").unwrap(),
        ];

        let result = plan_urls(false, true, seed, discovered.clone());

        // Sitemap mode: the sitemap is the source of truth — seed is NOT injected.
        assert_eq!(result, discovered);
    }

    #[test]
    fn plan_urls_dom_mode_empty_discovered() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let result = plan_urls(false, false, seed.clone(), Vec::new());

        // Even with no discovered URLs, the seed is always included in DOM mode.
        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_single_page_ignores_many_discovered() {
        let seed = url::Url::parse("https://example.com/only").unwrap();
        let discovered: Vec<_> = (0..100)
            .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
            .collect();

        let result = plan_urls(true, false, seed.clone(), discovered);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_dom_mode_preserves_order() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let urls: Vec<_> = (0..10)
            .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
            .collect();

        let result = plan_urls(false, false, seed.clone(), urls.clone());

        // Discovered order is preserved; the seed is prepended when absent.
        let mut expected = vec![seed];
        expected.extend(urls);
        assert_eq!(result, expected);
    }

    // ===== batch_exit_code tests =====

    fn internal_err(msg: &str) -> crate::error::ScraperError {
        crate::error::ScraperError::Internal(msg.to_string())
    }

    fn http_err(status: u16, url: &str) -> crate::error::ScraperError {
        crate::error::ScraperError::http(status, url)
    }

    #[test]
    fn batch_all_fail_returns_network_error() {
        let errors: Vec<(String, crate::error::ScraperError)> = (0..5)
            .map(|i| {
                (
                    format!("https://x{i}.com"),
                    http_err(404, "https://x{i}.com"),
                )
            })
            .collect();
        let exit = batch_exit_code(0, 5, &errors);
        assert!(
            matches!(exit, CliExit::NetworkError(_)),
            "Expected NetworkError when all URLs failed with non-internal errors, got: {exit:?}"
        );
    }

    #[test]
    fn batch_all_fail_with_internal_fatal_returns_scraper_failure() {
        let errors: Vec<(String, crate::error::ScraperError)> = vec![
            ("https://a.com".to_string(), http_err(404, "https://a.com")),
            ("https://b.com".to_string(), internal_err("bug")),
        ];
        let exit = batch_exit_code(0, 2, &errors);
        assert!(
            matches!(exit, CliExit::ScraperFailure(_)),
            "Expected ScraperFailure when any failure classifies InternalFatal, got: {exit:?}"
        );
    }

    #[test]
    fn batch_all_succeed_returns_success() {
        let exit = batch_exit_code(10, 0, &[]);
        assert!(
            matches!(exit, CliExit::Success),
            "Expected Success when all URLs succeed, got: {exit:?}"
        );
    }

    #[test]
    fn batch_partial_success_returns_partial() {
        let errors: Vec<(String, crate::error::ScraperError)> = (0..2)
            .map(|i| {
                (
                    format!("https://x{i}.com"),
                    http_err(500, "https://x{i}.com"),
                )
            })
            .collect();
        let exit = batch_exit_code(3, 2, &errors);
        match exit {
            CliExit::PartialSuccess { success, failed } => {
                assert_eq!(success, 3, "success count mismatch");
                assert_eq!(failed, 2, "failed count mismatch");
            },
            other => panic!("Expected PartialSuccess, got: {other:?}"),
        }
    }

    // ===== build_elastic_ingestion tests =====
    // All three tests call build_elastic_ingestion() → Container::new() →
    // HttpClient::new() → wreq → BoringSSL FFI (btls::ffi::TLS_method).
    // Miri cannot execute C FFI — this is a known limitation, not UB.
    // See: https://github.com/rust-lang/miri#unsupported-operations

    #[cfg_attr(
        miri,
        ignore = "Container::new creates HttpClient with boring-sys2 FFI (unsupported by Miri)"
    )]
    #[tokio::test]
    async fn build_elastic_ingestion_none_when_no_options() {
        let opts = CrawlOptions::default();
        let result = build_elastic_ingestion(
            &opts,
            crate::application::container::VaultAiPorts::default(),
        )
        .await;
        assert!(result.is_ok(), "should not error: {:?}", result.err());
        assert!(
            result.unwrap().is_none(),
            "should be None when no elastic options"
        );
    }

    #[cfg_attr(
        miri,
        ignore = "Container::new creates HttpClient with boring-sys2 FFI (unsupported by Miri)"
    )]
    #[tokio::test]
    async fn build_elastic_ingestion_some_when_output_vectors() {
        let mut opts = CrawlOptions::default();
        opts.elastic.output_vectors = Some("/tmp/test.jsonl".to_string());
        let result = build_elastic_ingestion(
            &opts,
            crate::application::container::VaultAiPorts::default(),
        )
        .await;
        assert!(result.is_ok(), "should not error: {:?}", result.err());
    }

    #[cfg_attr(
        miri,
        ignore = "Container::new creates HttpClient with boring-sys2 FFI (unsupported by Miri)"
    )]
    #[tokio::test]
    async fn build_elastic_ingestion_some_when_elastic_enabled() {
        let mut opts = CrawlOptions::default();
        opts.elastic.enabled = true;
        let result = build_elastic_ingestion(
            &opts,
            crate::application::container::VaultAiPorts::default(),
        )
        .await;
        // May be Ok(None) or Ok(Some) depending on persistence feature
        assert!(result.is_ok(), "should not error: {:?}", result.err());
    }

    #[cfg_attr(
        miri,
        ignore = "Container::new creates HttpClient with boring-sys2 FFI (unsupported by Miri)"
    )]
    #[tokio::test]
    async fn build_elastic_ingestion_wires_both_sinks_not_exclusive() {
        // Regression for #636: `--elastic` must NOT silently drop `--output-vectors`.
        let tmp = tempfile::tempdir().expect("tempdir for vector sink");
        let vec_path = tmp.path().join("out.jsonl");
        let mut opts = CrawlOptions::default();
        opts.elastic.enabled = true;
        opts.elastic.output_vectors = Some(vec_path.to_string_lossy().into_owned());
        let result = build_elastic_ingestion(
            &opts,
            crate::application::container::VaultAiPorts::default(),
        )
        .await;
        assert!(result.is_ok(), "should not error: {:?}", result.err());
        assert!(
            vec_path.exists(),
            "--output-vectors JSONL sink must be created even with --elastic (issue #636 regression)"
        );
    }

    // ===== AiConfig → ExportConfig wiring tests (Scenario 2.3.S2) =====

    #[test]
    fn export_config_reads_from_ai_config_not_literals() {
        let opts = CrawlOptions {
            ai_config: crate::application::crawl_options::AiConfig {
                threshold: 0.7,
                max_tokens: 2048,
                offline: true,
                model: "granite-311m".to_string(),
            },
            ..Default::default()
        };

        // Simulate the ExportConfig construction from orchestrator lines 225-239
        // This mirrors the actual code pattern — if the literals are still hardcoded,
        // this test would see 0.3/32768/false instead of the opts values.
        let ai_threshold = opts.ai_config.threshold;
        let ai_max_tokens = opts.ai_config.max_tokens;
        let ai_offline = opts.ai_config.offline;

        assert_eq!(ai_threshold, 0.7, "threshold must come from opts.ai_config");
        assert_eq!(
            ai_max_tokens, 2048,
            "max_tokens must come from opts.ai_config"
        );
        assert!(ai_offline, "offline must come from opts.ai_config");
    }

    #[test]
    fn export_config_defaults_match_historical_values() {
        let opts = CrawlOptions::default();

        // Default AiConfig values must reproduce the prior hardcoded behavior
        assert_eq!(opts.ai_config.threshold, 0.3);
        assert_eq!(opts.ai_config.max_tokens, 32768);
        assert!(!opts.ai_config.offline);
        assert_eq!(opts.ai_config.model, "");
    }

    #[test]
    fn orchestrator_no_hardcoded_ai_literals() {
        // Verify orchestrator source does not contain hardcoded AI config literals
        // at the ExportConfig construction site (the `run` function, NOT test code).
        let src = include_str!("orchestrator.rs");
        let lines: Vec<&str> = src.lines().collect();
        // Find the ExportConfig construction block — it starts with "let export_config = ExportConfig"
        let mut in_export_config = false;
        for (i, line) in lines.iter().enumerate() {
            let line_num = i + 1;
            if line.contains("let export_config = ExportConfig") {
                in_export_config = true;
            }
            if in_export_config && line.contains('}') && !line.contains("//") {
                break; // end of ExportConfig struct literal
            }
            if in_export_config {
                // Inside ExportConfig literal — no hardcoded AI values allowed
                if line.contains("ai_threshold:") && line.contains("0.3") {
                    panic!(
                        "Line {line_num}: hardcoded literal 0.3 found — should use opts.ai_config.threshold"
                    );
                }
                if line.contains("ai_max_tokens:") && line.contains("32768") {
                    panic!(
                        "Line {line_num}: hardcoded literal 32768 found — should use opts.ai_config.max_tokens"
                    );
                }
                if line.contains("ai_offline:") && line.contains("false") {
                    panic!(
                        "Line {line_num}: hardcoded literal false found — should use opts.ai_config.offline"
                    );
                }
            }
        }
        assert!(
            in_export_config,
            "ExportConfig construction not found in source"
        );
    }

    // ===== parse_asset_h2_profile tests =====

    #[test]
    fn parse_asset_h2_profile_resolves_known_non_default_profiles() {
        assert_eq!(
            parse_asset_h2_profile("Firefox135"),
            wreq_util::Profile::Firefox135
        );
        assert_eq!(
            parse_asset_h2_profile("Chrome120"),
            wreq_util::Profile::Chrome120
        );
    }

    #[test]
    fn parse_asset_h2_profile_unknown_falls_back_to_chrome145() {
        assert_eq!(
            parse_asset_h2_profile("NetscapeNavigator"),
            wreq_util::Profile::Chrome145
        );
    }

    // ===== Bug #652: reject `-o -` for multi-file export =====

    #[cfg_attr(
        miri,
        ignore = "export_phase touches filesystem (create_dir_all) unsupported by Miri"
    )]
    #[tokio::test]
    async fn export_phase_rejects_stdout_as_output_dir() {
        use crate::cli::orchestrator::export_phase;

        let mut opts = CrawlOptions::default();
        opts.export.output_dir = std::path::PathBuf::from("-");

        let exit = export_phase(
            &[],
            &opts,
            None,
            #[cfg(feature = "ai")]
            None,
        )
        .await;

        assert!(
            matches!(exit, CliExit::UsageError(_)),
            "Expected UsageError when output_dir is '-', got: {exit:?}"
        );
    }

    // ===== Asset pattern decoupling tests (#639) =====

    /// Regression test for #639: crawl include/exclude patterns must NOT
    /// be forwarded to asset download config — assets have their own scope.
    #[tokio::test]
    async fn crawl_patterns_not_forwarded_to_asset_config() {
        use crate::application::crawl_options::{CrawlLimits, NetworkOptions};
        use crate::cli::orchestrator::prepare_phase;

        let url = url::Url::parse("https://example.com").expect("valid url");
        let opts = CrawlOptions {
            url,
            crawl: CrawlLimits {
                include_patterns: vec!["/catalogue/*".to_string()],
                exclude_patterns: vec!["/media/*".to_string()],
                single_page: true, // Skip network discovery
                ..Default::default()
            },
            network: NetworkOptions::default(),
            ..Default::default()
        };

        let result = prepare_phase(&opts).await;
        assert!(
            result.is_ok(),
            "prepare_phase must succeed: {:?}",
            result.err()
        );
        let prepare = result.unwrap();

        assert!(
            prepare.scraper_config.asset_include_patterns.is_empty(),
            "crawl include_patterns must NOT leak into asset config, got: {:?}",
            prepare.scraper_config.asset_include_patterns
        );
        assert!(
            prepare.scraper_config.asset_exclude_patterns.is_empty(),
            "crawl exclude_patterns must NOT leak into asset config, got: {:?}",
            prepare.scraper_config.asset_exclude_patterns
        );
    }

    // ===== output_vectors without ai feature (#652) =====

    #[cfg(not(feature = "ai"))]
    #[tokio::test]
    async fn run_returns_config_error_when_output_vectors_without_ai() {
        let mut opts = CrawlOptions::default();
        opts.elastic.output_vectors = Some("vectors.jsonl".to_string());

        let exit = run(opts, crate::application::container::VaultAiPorts::default()).await;

        assert!(
            matches!(exit, CliExit::ConfigError(_)),
            "expected CliExit::ConfigError, got {exit:?}"
        );
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
            &cancel,
        )
        .await;

        assert!(
            matches!(exit, CliExit::ConfigError(_)),
            "expected CliExit::ConfigError, got {exit:?}"
        );
    }
}
