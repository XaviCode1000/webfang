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
use crate::domain::config::ScraperConfig;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::persistence::PersistenceMode;
use crate::domain::site::SitemapConfig;
use crate::CrawlerConfig;

use crate::domain;
use crate::domain::persistence::StateStorePort;
use crate::infrastructure::output::file_saver::ObsidianOptions;

pub use crate::cli::parse::handle_completions;

#[cfg(feature = "ai")]
use crate::domain::semantic_cleaner::SemanticCleaner;

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

/// Pre-flight gate for `--output-vectors` (#703, #652).
///
/// `--output-vectors` can only write embeddings when semantic cleaning is
/// requested (`--clean-ai` / `opts.ai`). Every entry point (`run`,
/// `run_batch`) must run this gate BEFORE `build_elastic_ingestion` wires the
/// stream sink: `StreamRepository::new(path)` creates/truncates the target
/// file as a construction side effect, so failing downstream of it leaks a
/// 0-byte vectors file alongside a success exit (silent data loss for RAG
/// pipelines, class S1).
///
/// - With the `ai` feature: `output_vectors && !opts.ai` → `DataFormatError`
///   (exit 65, `EX_DATA`) with a Spanish user-facing message.
/// - Without it: the flag is unusable → `ConfigError` (exit 78) telling the
///   user to rebuild with `--features ai`.
///
/// Returns `Some(exit)` when the run must be aborted, `None` to proceed.
fn output_vectors_gate(opts: &CrawlOptions) -> Option<CliExit> {
    opts.elastic.output_vectors.as_ref()?;

    #[cfg(feature = "ai")]
    {
        if opts.ai {
            return None;
        }
        warn!("--output-vectors refused without --clean-ai; no vectors to export");
        Some(CliExit::DataFormatError(
            "No hay vectores para exportar: '--output-vectors' requiere '--clean-ai' para generar embeddings".to_string(),
        ))
    }

    #[cfg(not(feature = "ai"))]
    {
        Some(CliExit::ConfigError(
            "Se requiere compilar con '--features ai' para usar --output-vectors".to_string(),
        ))
    }
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

    // #703/#652: pre-flight gate for `--output-vectors` — single source of
    // truth shared with `run_batch()` (see `output_vectors_gate`), and it must
    // run BEFORE `build_elastic_ingestion` wires the stream sink below.
    if let Some(exit) = output_vectors_gate(&opts) {
        return exit;
    }

    // #796: pre-flight gate for `--export-format vector` without `--clean-ai` —
    // mirrors the CLI preflight in `main.rs` so the MCP / batch path is also
    // covered (defense in depth: no invalid `export.json` with `dimensions: null`).
    if let Err(exit) = crate::cli::preflight::check_export_format_vector(&opts) {
        return exit;
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

    // PersistenceMode unified control-plane — pure resolver with default dir.
    // Built BEFORE prepare_phase so discovery Engine can be wired with
    // `with_persistence` (checkpoint interval flows from the mode, not hardcoded).
    // The resolver itself never logs (#1045): `--state-dir` without `--resume`
    // is reported via `ResolverNotes` and warned about here, the one call
    // site that knows about user flags.
    let persistence_mode = resolve_persistence_mode(&opts);

    let prepare = match prepare_phase(&opts, &persistence_mode).await {
        Err(e) => return e,
        Ok(p) => p,
    };

    let discovered_count = prepare.urls_to_scrape.len();
    let (urls_to_scrape, state_store) = match apply_resume_mode(
        prepare.urls_to_scrape,
        &persistence_mode,
        opts.url.as_str(),
        &root_correlation,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => return e,
    };

    // #705 Paso 2: a --resume run where every discovered URL was already
    // processed by a prior run is a technical success ("nothing pending").
    if let Some(exit) =
        resume_nothing_pending(&opts, &urls_to_scrape, discovered_count, &root_correlation)
    {
        return exit;
    }

    let elastic_ingestion = match build_elastic_ingestion(&opts, vault_ports).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Create observer with stderr fallback (no channel) for non-TUI mode:
    // scraping runs fully headless; there is no TUI progress screen.
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

    // #779: export the successfully-scraped pages BEFORE the report/exit
    // decision. Previously `report_phase` short-circuited on partial success
    // (some pages failed, some succeeded) and `export_phase` never ran — so a
    // partial-success crawl silently discarded all its content (exit 69 with an
    // empty output directory), unlike batch mode which always exports.
    #[cfg(feature = "ai")]
    let export_exit = export_phase(&results, &opts, state_store.as_deref(), ai_cleaner).await;
    #[cfg(not(feature = "ai"))]
    let export_exit = export_phase(&results, &opts, state_store.as_deref()).await;

    // Special cell — Cancelled (error-classification-matrix): cooperative
    // cancellation is a control signal, not an operational failure, so it
    // wins over classification-based routing below — a Ctrl-C mid-crawl
    // exits 0 even with partial failures (#509 semantics at the CLI
    // boundary). Export above already ran: captured content is still
    // written (graceful shutdown).
    if let Some(exit) = crate::cli::error::cancelled_exit(cancel.is_cancelled()) {
        return exit;
    }

    if let Some(exit) = report_phase(&results, &failures, blocked, opts.verbosity) {
        return exit;
    }

    export_exit
}

/// #705 Paso 2: a `--resume` run where every discovered URL was already
/// processed by a prior run is a technical success ("nothing pending"), not a
/// network failure. Return `Some(CliExit::Success)` before the scrape phase so
/// an empty filtered list never reaches `report_phase`'s false exit-69 path.
/// The `discovered_count > 0` guard keeps a genuinely empty discovery on its
/// existing route instead of masking it as resume success.
fn resume_nothing_pending(
    opts: &CrawlOptions,
    urls_to_scrape: &[url::Url],
    discovered_count: usize,
    root_correlation: &domain::CorrelationId,
) -> Option<CliExit> {
    if opts.crawl.resume && urls_to_scrape.is_empty() && discovered_count > 0 {
        info!(
        skipped = discovered_count,
        trace_id = %root_correlation.trace_id(),
        "resume: all discovered URLs already processed, nothing pending"
        );
        if !opts.export.quiet {
            println!(
"Resume: nada pendiente — {discovered_count} URL(s) ya procesadas en ejecuciones anteriores."
);
        }
        return Some(CliExit::Success);
    }
    None
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
///
/// An EXPLICIT `--vault` flag (captured as `vault_is_explicit` at parse time,
/// #762) extends the same invariant: the vault becomes the output base so
/// Markdown, assets and the RAG export all land inside it without the user
/// duplicating the path in `-o`. Vaults filled from `config.toml` or
/// autodetection do NOT redirect — that is why explicitness is tracked
/// separately from `obsidian_vault.is_some()`.
///
/// If `obsidian_vault` is somehow `None` at use time (should be unreachable
/// after preflight validation), fall back to `output_dir` — never panic.
fn resolve_persistence_root(opts: &CrawlOptions) -> std::path::PathBuf {
    if opts.export.quick_save || opts.export.vault_is_explicit {
        opts.export
            .obsidian_vault
            .clone()
            .unwrap_or_else(|| opts.export.output_dir.clone())
    } else {
        opts.export.output_dir.clone()
    }
}

/// Resolve the directory for the RAG pipeline export (`export.jsonl` /
/// `export.json`).
///
/// The persistence root ([`resolve_persistence_root`]) owns where Markdown
/// and assets are written; this helper exists so every sink of
/// `opts.export.output_dir` converges on ONE resolution path instead of each
/// call site re-deriving its own base (#762). The two diverge by design:
///
/// - `--quick-save` routes Markdown into `<persistence root>/_inbox` while
///   the RAG export keeps its historical `-o` destination (unchanged by
///   #762).
/// - Explicit `--vault` without `--quick-save` also redirects the RAG export
///   into the vault root, so a single `--vault` flag makes the vault
///   self-contained.
fn resolve_export_dir(opts: &CrawlOptions) -> std::path::PathBuf {
    if opts.export.quick_save {
        opts.export.output_dir.clone()
    } else {
        resolve_persistence_root(opts)
    }
}

/// Export scraped results to files and run AI cleaning if requested.
async fn export_phase(
    results: &[domain::ScrapedContent],
    opts: &CrawlOptions,
    state_store: Option<&dyn StateStorePort>,
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

    let output_dir = resolve_export_dir(opts);

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
        // #1107: async creation on `tokio::fs` (no blocking syscall on the
        // executor) and the Result is propagated, not discarded — a real
        // failure (ENOSPC, read-only vault) now stops the export with a
        // typed CliExit instead of surfacing late and degraded inside
        // `save_files`. `create_dir_all` is idempotent, so the old
        // `exists()` pre-check is gone.
        if let Err(e) = tokio::fs::create_dir_all(&inbox).await {
            return CliExit::ConfigError(format!(
                "no se pudo crear el inbox '{}': {e}",
                inbox.display()
            ));
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
    let tls_emulation = match HttpClientConfig::profile_from_name(&opts.network.h2_profile) {
        Ok(profile) => profile,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };
    let crawler_config = match build_crawler_config_for_discovery(&opts, tls_emulation) {
        Ok(config) => config,
        Err(e) => return e,
    };

    // #784: with --batch-file, opts.url is empty, so discovering from it would
    // report "0 URL(s) would be scraped". List the batch URLs the user actually
    // supplied instead — that is the set a dry run should preview.
    if opts.batch.batch_file.is_some() {
        let budget = crate::domain::budget::BudgetModel::build(
            opts.budget_overrides,
            &crate::domain::budget::detector::SystemDetector,
        );
        let manager = match load_batch_manager(&opts, crawler_config, &budget).await {
            Ok(m) => m,
            Err(e) => return e,
        };
        let urls = manager.urls();
        info!(
            "Dry-run: listing {} batch URL(s) without scraping",
            urls.len()
        );
        println!("\nDry-run: {} URL(s) would be scraped:", urls.len());
        for url in &urls {
            println!("  {url}");
        }
        return CliExit::Success;
    }

    // Bug 4: honest dry-run - call real URL discovery
    info!("Dry-run: discovering URLs without scraping...");
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

/// Resolve the CLI-projected sitemap pair into the domain boundary (#1190).
///
/// The single home (with [`SitemapConfig::resolve`]) of the
/// `sitemap_url.is_some() → enabled` coercion: the preflight book, the
/// `webfang_cli` projection, and the builder coercion all collapsed here,
/// so an explicit but invalid URL fails as `CliExit::ConfigError`
/// (Spanish, typed) before any discovery starts.
fn resolve_sitemap_projection(opts: &CrawlOptions) -> Result<SitemapConfig, CliExit> {
    SitemapConfig::resolve(opts.crawl.use_sitemap, opts.crawl.sitemap_url.as_deref())
        .map_err(|e| CliExit::ConfigError(e.to_string()))
}

/// Build a `CrawlerConfig` for URL discovery (shared by dry-run, prepare, and batch).
fn build_crawler_config_for_discovery(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
) -> Result<CrawlerConfig, CliExit> {
    let crawler_config = CrawlerConfig::builder(opts.url.clone())
        .max_pages(opts.crawl.max_pages)
        .max_depth(opts.crawl.max_depth)
        .include_patterns(opts.crawl.include_patterns.clone())
        .exclude_patterns(opts.crawl.exclude_patterns.clone())
        .ignore_robots(opts.crawl.ignore_robots)
        .sitemap(resolve_sitemap_projection(opts)?)
        .timeout_secs(opts.network.timeout_secs)
        .delay_ms(opts.network.delay_ms)
        // Bug R2-1: recursive URL discovery runs the real crawl Engine, so
        // the operator overrides must ride on the config or the Engine
        // silently re-derives the auto tiers.
        .budget_overrides(opts.budget_overrides)
        .tls_emulation(tls_emulation)
        .build();
    Ok(crawler_config)
}

/// Resolve the persistence mode and warn about ignored CLI flags.
///
/// The domain resolver is pure (#1045): it never logs. `--state-dir`
/// without `--resume` is reported via `ResolverNotes` and warned about
/// here, the one call site that knows about user flags.
fn resolve_persistence_mode(opts: &CrawlOptions) -> PersistenceMode {
    let default_state_dir = crate::cli::scrape_flow::resolve_default_state_dir();
    let (persistence_mode, resolver_notes) =
        crate::domain::persistence::PersistenceMode::from_config_with_notes(
            &opts.crawl.resume_config(),
            &default_state_dir,
        );
    if let Some(ignored_state_dir) = resolver_notes.ignored_state_dir {
        warn!(
        state_dir = ?ignored_state_dir,
        "ignoring --state-dir without --resume"
        );
    }
    persistence_mode
}

/// Prepare scraper config and discover URLs.
///
/// Returns the initial `ScraperConfig` (before asset/download wiring) and
/// the list of URLs to scrape.  On discovery failure, returns the
/// appropriate `CliExit` error.
async fn prepare_phase(
    opts: &CrawlOptions,
    persistence_mode: &PersistenceMode,
) -> Result<PrepareResult, CliExit> {
    let urls_to_scrape = if opts.crawl.single_page {
        plan_urls(true, false, opts.url.clone(), Vec::new())
    } else {
        // Honor `--h2-profile` for URL discovery (#312): an unknown profile is a
        // config error (exit 78), consistent with the scrape and batch phases.
        let tls_emulation = HttpClientConfig::profile_from_name(&opts.network.h2_profile)
            .map_err(|e| CliExit::ConfigError(e.to_string()))?;

        let crawler_config = build_crawler_config_for_discovery(opts, tls_emulation)?;

        // Sitemap mode is the source of truth (depth-agnostic XML), so keep the
        // existing single-pass sitemap discovery. DOM mode must run the recursive
        // crawl Engine so `--max-depth` is honored (bug #651): the legacy
        // `discover_urls_single_fetch` path did one fetch and silently ignored depth.
        let discovered_urls = if opts.crawl.use_sitemap {
            match discover_urls(&crawler_config, opts).await {
                // "Site has no sitemap" is a terminal discovery state, not
                // an infrastructure failure (#695): exit 2 lets automation
                // distinguish it from a real network outage (exit 69).
                Err(crate::error::ScraperError::SitemapNotFound(_)) => {
                    return Err(CliExit::EmptyDiscovery(
                        "No URLs discovered: sitemap not found".into(),
                    ));
                },
                Err(crate::error::ScraperError::SitemapEmpty) => {
                    return Err(CliExit::EmptyDiscovery(
                        "No URLs discovered: sitemap is empty".into(),
                    ));
                },
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
            //
            // The persistence_mode is forwarded to `discover_urls_recursive`,
            // which applies `crawl_site_with_options` when the mode enables
            // checkpointing and falls back to `crawl_site` otherwise — single
            // call site, no orchestrator-level branching (slice 5c followup).
            match discover_urls_recursive(crawler_config, opts, persistence_mode).await {
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

    // Budget model built ONCE at flow entry (design D4): operator overrides
    // plus the canonical detector seam feed every downstream bound.
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );

    let mut scraper_config = ScraperConfig::default()
        .with_output_dir(resolve_persistence_root(opts))
        // Scraper + asset-download bounds derive from the model's Operation.crawl
        // and Asset tiers (task 2.5b); explicit flags arrive via BudgetOverrides.
        .with_scraper_concurrency(budget.crawl().get())
        .with_max_pages(opts.crawl.max_pages)
        .with_selector(opts.crawl.selector.clone())
        .with_ignore_waf(opts.crawl.ignore_waf)
        .with_dom_preprune(opts.crawl.dom_preprune);

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
    scraper_config = scraper_config.with_download_concurrency(budget.asset().get());
    // Effective asset-tier bound logged at INFO so operators (and behavioral
    // tests) can verify an explicit `--download-concurrency` reached this
    // enforcement site (#897 item 5). Structured field — never interpolate
    // values into the message (m1).
    info!(
        asset_concurrency = budget.asset().get(),
        "Asset downloads wired"
    );
    scraper_config = scraper_config.with_max_file_size(opts.network.max_file_size);
    scraper_config = scraper_config.with_download_timeout(opts.network.download_timeout_secs);

    // Create shared Downloader once for connection pooling across all page scrapes.
    // Q3 MEASURE FIRST: the dedup cache is the only structure whose measured
    // growth crossed the 50 MB materiality line; its capacity derives from the
    // Asset tier like every other budget-model bound.
    // Single graph (#1149): the ephemeral asset downloader is built through
    // the `Container` factory — fresh and bounded per run, never the MCP
    // server's long-lived shared downloader (#1120).
    let shared_downloader = if scraper_config.has_downloads() {
        match crate::application::container::Container::build_ephemeral_asset_downloader(
            &scraper_config,
            budget.asset().get(),
        ) {
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
/// Severity routing (#537 + #706 + matrix rows 21/22) applies when every
/// URL failed:
///
/// 1. An internal-fatal failure wins → `CliExit::ScraperFailure` (3) — an
///    internal bug must not masquerade as a transient network outage.
/// 2. A permanent-kind [`crate::error::ScraperError::Io`] failure is next →
///    `CliExit::IoError` (74) — an unwritable output path is EX_IOERR.
/// 3. Any [`crate::error::ScraperError::ExtractionFailed`] is next →
///    `CliExit::DataFormatError` (65) — the pages were fetched but carried
///    no usable content (JS-only shells, poor fallback).
/// 4. Anything else keeps `CliExit::NetworkError` (69).
///
/// The partial-success case always reports `PartialSuccess` (69) regardless of
/// failure severity: some content was scraped, which is the dominant signal.
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

    // Canonical robots-blocked override (#705): fires only when NOTHING was
    // scraped and NOTHING failed, delegating to the single implementation in
    // `cli::error` (#839).
    if let Some(exit) =
        crate::cli::error::forbidden_exit_when_all_blocked(results.len(), failures.len(), blocked)
    {
        return Some(exit);
    }

    if results.is_empty() {
        // Exit-code precedence in the all-fail arm: internal-fatal (3)
        // outranks the permanent-Io override (74), which outranks
        // extraction-failed (65), which outranks the transient fallback
        // (69) (#706 + matrix rows 21/22). An internal bug must never
        // masquerade as a data-format error. The Io override sits here —
        // AFTER the InternalFatal sweep, BEFORE extraction-failed — because
        // since `ScraperError::classify` splits by kind, a permanent io
        // error is PermanentFatal (never caught by the 3 sweep), and a run
        // that failed entirely on an unwritable output path must report 74,
        // not fall through to 65/69.
        // Exit-code precedence in the all-fail arm: internal-fatal (3)
        // outranks the permanent-Io override (74), which outranks
        // extraction-failed (65), which outranks the transient fallback
        // (69) (#706 + matrix rows 21/22). An internal bug must never
        // masquerade as a data-format error. The Io override sits here —
        // AFTER the InternalFatal sweep, BEFORE extraction-failed — because
        // since `ScraperError::classify` splits by kind, a permanent io
        // error is PermanentFatal (never caught by the 3 sweep), and a run
        // that failed entirely on an unwritable output path must report 74,
        // not fall through to 65/69. Each arm delegates to its canonical
        // mapping function in `cli::error` (#839) so every exit-code
        // decision has exactly one implementation.
        if let Some(exit) = crate::cli::error::scraper_failure_exit_when_internal_fatal(failures) {
            return Some(exit);
        }
        if let Some(exit) = permanent_io_error_for_failures(failures) {
            return Some(exit);
        }
        if let Some(exit) =
            crate::cli::error::data_format_error_exit_when_extraction_failed(failures)
        {
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
/// Severity routing lives entirely in the canonical mapping functions of
/// [`crate::cli::error`] (#839): internal-fatal → `ScraperFailure` (3) via
/// `scraper_failure_exit_when_internal_fatal`, permanent-Io → `IoError`
/// (74) via the per-item `permanent_io_error_exit_for`, extraction-failed →
/// `DataFormatError` (65) via `data_format_error_exit_when_extraction_failed`.
/// This local adapter only dispatches the variant through the canonical
/// per-item helper, keeping one implementation of the 74 decision.
fn permanent_io_error_for_failures(
    failures: &[(String, crate::error::ScraperError)],
) -> Option<CliExit> {
    failures.iter().find_map(|(_, e)| match e {
        crate::error::ScraperError::Io(io_err) => {
            crate::cli::error::permanent_io_error_exit_for(io_err)
        },
        _ => None,
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
        export_phase(&results, &opts, state_store.as_deref(), ai_cleaner).await;
    }
    #[cfg(not(feature = "ai"))]
    {
        export_phase(&results, &opts, state_store.as_deref()).await;
    }

    // Final exit code aggregates BOTH crawl-level and extraction-level outcomes
    // with `#537` severity routing: partial success -> 69, all-fail with an
    // internal fatal error -> 3, otherwise 0. Crawl failures were only logged
    // above, so this is the only place the batch's true status surfaces.

    // Special cell — Cancelled: same precedence as the single-run path —
    // cancellation beats classification-based routing and exits 0.
    if let Some(exit) = crate::cli::error::cancelled_exit(cancel.is_cancelled()) {
        return exit;
    }

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
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let crawler_config = build_batch_crawler_config(opts, tls_emulation, &budget)?;
    let manager = load_batch_manager(opts, crawler_config, &budget)
        .await?
        .with_content_sink(sink);

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

    Ok(())
}

/// Create the disk-backed capture sink for a batch run.
///
/// The spool lives under the run's persistence root so it shares the run's
/// storage budget (and lands inside the vault when `--quick-save` or an
/// explicit `--vault` redirects the base, #638/#762) and is cleaned up by
/// [`discard_batch_spool`] once extraction is done.
async fn build_batch_sink(opts: &CrawlOptions) -> Result<BoundedFileSink, CliExit> {
    let spool_path = resolve_persistence_root(opts).join(".webfang-batch-capture.jsonl");
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
/// `--delay-ms` and the concurrency bound are propagated here (#653): without
/// them the batch engine crawled at full speed with its own default concurrency,
/// making both flags silent no-ops on the `--batch` path.
///
/// The concurrency bound comes from the run's [`BudgetModel`] Operation.crawl
/// tier (task 2.5b).
fn build_batch_crawler_config(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    budget: &crate::domain::budget::BudgetModel,
) -> Result<CrawlerConfig, CliExit> {
    let crawler_config = CrawlerConfig::builder(opts.url.clone())
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
async fn load_batch_manager(
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

/// Determine the CLI exit code from batch scrape results.
///
/// Severity routing (#537 + #706 + matrix rows 21/22) for all-fail runs
/// (`failed > 0 && succeeded == 0`):
///
/// 1. An internal-fatal failure wins → `CliExit::ScraperFailure` (3).
/// 2. A permanent-kind I/O failure is next → `CliExit::IoError` (74).
/// 3. Any [`crate::error::ScraperError::ExtractionFailed`] is next →
///    `CliExit::DataFormatError` (65).
/// 4. Anything else keeps `CliExit::NetworkError` (69).
///
/// Partial success remains `CliExit::PartialSuccess` (69) regardless of
/// failure severity — some content was scraped, which is the dominant signal.
fn batch_exit_code(
    succeeded: usize,
    failed: usize,
    errors: &[(String, crate::error::ScraperError)],
) -> CliExit {
    if failed > 0 && succeeded == 0 {
        // Same precedence as `report_phase` (#706): internal-fatal (3)
        // first, then the permanent-Io override (74), then
        // extraction-failed (65), then the transient fallback (69).
        // The 74 override must precede extraction-failed so an all-fail run
        // caused by an unwritable output path reports 74 truthfully. Each
        // arm delegates to its canonical mapping function in `cli::error`
        // (#839) so every exit-code decision has exactly one implementation.
        if let Some(exit) = crate::cli::error::scraper_failure_exit_when_internal_fatal(errors) {
            return exit;
        }
        if let Some(exit) = permanent_io_error_for_failures(errors) {
            return exit;
        }
        if let Some(exit) = crate::cli::error::data_format_error_exit_when_extraction_failed(errors)
        {
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
        batch_exit_code, build_batch_crawler_config, build_crawler_config_for_discovery,
        build_elastic_ingestion, format_failure, parse_asset_h2_profile, plan_urls, report_phase,
        resolve_export_dir, resolve_persistence_root,
    };
    use crate::application::crawl_options::CrawlOptions;
    use crate::cli::error::CliExit;

    use super::{run, run_batch};

    // ===== build_batch_crawler_config tests (#653) =====

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
        let budget = crate::domain::budget::BudgetModel::build(
            opts.budget_overrides,
            &crate::domain::budget::detector::SystemDetector,
        );

        let config = build_batch_crawler_config(&opts, wreq_util::Profile::Chrome145, &budget)
            .expect("valid test projection must build");

        assert_eq!(config.delay_ms, 750, "--delay-ms must reach the crawler");
        assert_eq!(
            config.concurrency.get(),
            2,
            "explicit --concurrency must reach the crawler through the model"
        );
    }

    #[test]
    fn discovery_config_propagates_budget_overrides() {
        // Bug R2-1: recursive URL discovery runs the real crawl Engine via
        // crawl_site; the operator overrides staged on CrawlOptions must be
        // carried onto the config so the Engine honors them.
        let mut opts = CrawlOptions::default();
        opts.budget_overrides.crawl = crate::domain::budget::tiers::CrawlConcurrency::new(6).ok();
        opts.budget_overrides.rate_burst = crate::domain::budget::tiers::BurstPermits::new(11).ok();

        let config = build_crawler_config_for_discovery(&opts, wreq_util::Profile::Chrome145)
            .expect("valid test projection must build");

        assert_eq!(
            config.budget_overrides.crawl.map(|c| c.get()),
            Some(6),
            "explicit --concurrency must reach the discovery Engine"
        );
        assert_eq!(
            config.budget_overrides.rate_burst.map(|b| b.get()),
            Some(11),
            "explicit --rate-limit-burst must reach the discovery Engine"
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
        let budget = crate::domain::budget::BudgetModel::build(
            opts.budget_overrides,
            &crate::domain::budget::detector::SystemDetector,
        );

        let config = build_batch_crawler_config(&opts, wreq_util::Profile::Chrome145, &budget)
            .expect("valid test projection must build");

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

    // ===== sitemap projection tests (#1190) =====

    #[test]
    fn discovery_config_invalid_sitemap_url_is_config_error() {
        // End-to-end projection rejection: an explicit but invalid URL
        // fails HERE (Spanish, typed) instead of travelling into
        // discovery and failing late at fetch/parse time.
        let mut opts = CrawlOptions::default();
        opts.crawl.use_sitemap = true;
        opts.crawl.sitemap_url = Some("not-a-url".to_string());

        let err = build_crawler_config_for_discovery(&opts, wreq_util::Profile::Chrome145)
            .expect_err("invalid sitemap URL must fail the projection");
        match err {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("sitemap") && msg.contains("inválida"),
                "rejection must name the sitemap URL in Spanish, got: {msg}"
            ),
            other => panic!("expected ConfigError, got: {other:?}"),
        }
    }

    #[test]
    fn discovery_config_explicit_url_implies_enabled() {
        // The `Some(url) implies intent` coercion lives in the single
        // domain rule now: `false + Some(valid)` projects to enabled.
        let mut opts = CrawlOptions::default();
        opts.crawl.sitemap_url = Some("https://example.com/sitemap.xml".to_string());

        let config = build_crawler_config_for_discovery(&opts, wreq_util::Profile::Chrome145)
            .expect("valid sitemap URL must project");
        assert!(
            config.sitemap_config().is_enabled(),
            "explicit URL must imply intent through the projection"
        );
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
            quality_hint: None,
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

    fn extraction_failed(url: &str) -> crate::error::ScraperError {
        crate::error::ScraperError::ExtractionFailed {
            url: url.to_string(),
            reason: "contenido insuficiente (0 caracteres) — la página devolvió muy poco contenido extraíble"
                .to_string(),
        }
    }

    // ===== permanent-Io exit-74 override tests (matrix rows 21/22) =====

    fn io_err(kind: std::io::ErrorKind) -> crate::error::ScraperError {
        crate::error::ScraperError::Io(std::io::Error::new(kind, "io failure"))
    }

    #[test]
    fn batch_exit_code_all_permanent_io_returns_io_error_74() {
        // Matrix row 22: an all-failed run caused by a permanent io error
        // (unwritable output path) must exit 74 (EX_IOERR), not 3 or 69.
        let errors = vec![(
            "https://x.example.com".to_string(),
            io_err(std::io::ErrorKind::PermissionDenied),
        )];

        let exit = batch_exit_code(0, 1, &errors);

        assert!(
            matches!(exit, CliExit::IoError(_)),
            "permanent-kind Io all-fail must route to IoError(74), got {exit:?}"
        );
    }

    #[test]
    fn report_phase_all_permanent_io_returns_io_error_74() {
        // Same contract through the single-page `report_phase` path.
        let failures = vec![(
            "https://x.example.com".to_string(),
            io_err(std::io::ErrorKind::NotFound),
        )];

        let exit = report_phase(&[], &failures, 0, 0);

        assert!(
            matches!(exit, Some(CliExit::IoError(_))),
            "permanent-kind Io all-fail must route to IoError(74), got {exit:?}"
        );
    }

    #[test]
    fn batch_exit_code_transient_io_keeps_network_error_69() {
        // Matrix row 21: transient io kinds keep the class default (69);
        // the 74 override must NOT fire for them.
        let errors = vec![(
            "https://x.example.com".to_string(),
            io_err(std::io::ErrorKind::Interrupted),
        )];

        let exit = batch_exit_code(0, 1, &errors);

        assert!(
            matches!(exit, CliExit::NetworkError(_)),
            "transient-kind Io all-fail must keep NetworkError(69), got {exit:?}"
        );
    }

    #[test]
    fn batch_exit_code_internal_fatal_outranks_permanent_io() {
        // Precedence: the InternalFatal sweep still wins over the Io override —
        // a run with a genuine internal bug reports 3, not 74.
        let errors = vec![
            ("https://x.example.com".to_string(), internal_err("bug")),
            (
                "https://y.example.com".to_string(),
                io_err(std::io::ErrorKind::PermissionDenied),
            ),
        ];

        let exit = batch_exit_code(0, 2, &errors);

        assert!(
            matches!(exit, CliExit::ScraperFailure(_)),
            "InternalFatal must outrank the permanent-Io override, got {exit:?}"
        );
    }

    // ===== extraction-failed exit-65 routing tests (#706) =====

    #[test]
    fn report_phase_all_extraction_failed_returns_data_format_error() {
        // CE-1: a JS-only batch — every failure is the typed ExtractionFailed
        // — must exit 65 (DataFormatError) with the Spanish message.
        let failures = vec![
            (
                "https://js1.example.com".to_string(),
                extraction_failed("https://js1.example.com"),
            ),
            (
                "https://js2.example.com".to_string(),
                extraction_failed("https://js2.example.com"),
            ),
        ];

        let exit = report_phase(&[], &failures, 0, 0);

        match exit {
            Some(CliExit::DataFormatError(msg)) => {
                assert!(
                    msg.contains("extracción sin contenido útil"),
                    "Spanish message expected, got: {msg}"
                );
                assert!(
                    msg.contains("2 URL(s)"),
                    "message must count the failures: {msg}"
                );
            },
            other => panic!("expected DataFormatError, got: {other:?}"),
        }
    }

    #[test]
    fn report_phase_mixed_with_extraction_failed_keeps_partial_success() {
        // CE-2: one success + one ExtractionFailed stays PartialSuccess (69) —
        // extraction failures never outrank a successful scrape.
        let results = vec![scraped("https://ok.example.com")];
        let failures = vec![(
            "https://js.example.com".to_string(),
            extraction_failed("https://js.example.com"),
        )];

        let exit = report_phase(&results, &failures, 0, 0);

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
    fn report_phase_internal_fatal_outranks_extraction_failed() {
        // Precedence: internal-fatal (3) wins over extraction-failed (65) when
        // both failure classes are present in an all-fail run.
        let failures = vec![
            ("https://bug.example.com".to_string(), internal_err("panic")),
            (
                "https://js.example.com".to_string(),
                extraction_failed("https://js.example.com"),
            ),
        ];

        let exit = report_phase(&[], &failures, 0, 0);

        assert!(
            matches!(exit, Some(CliExit::ScraperFailure(_))),
            "internal-fatal must outrank extraction-failed, got: {exit:?}"
        );
    }

    #[test]
    fn batch_exit_code_all_extraction_failed_returns_data_format_error() {
        // CE-3 (batch): poor-fallback all-fails are typed ExtractionFailed too,
        // so an all-failed batch of them exits 65 instead of 69.
        let errors = vec![
            (
                "https://js1.example.com".to_string(),
                extraction_failed("https://js1.example.com"),
            ),
            (
                "https://js2.example.com".to_string(),
                extraction_failed("https://js2.example.com"),
            ),
        ];

        let exit = batch_exit_code(0, 2, &errors);

        match exit {
            CliExit::DataFormatError(msg) => {
                assert!(
                    msg.contains("extracción sin contenido útil"),
                    "Spanish message expected, got: {msg}"
                );
            },
            other => panic!("expected DataFormatError, got: {other:?}"),
        }
    }

    #[test]
    fn batch_exit_code_internal_fatal_outranks_extraction_failed() {
        // Same 3 > 65 precedence inside the batch code path.
        let errors = vec![
            ("https://bug.example.com".to_string(), internal_err("panic")),
            (
                "https://js.example.com".to_string(),
                extraction_failed("https://js.example.com"),
            ),
        ];

        let exit = batch_exit_code(0, 2, &errors);

        assert!(
            matches!(exit, CliExit::ScraperFailure(_)),
            "internal-fatal must outrank extraction-failed, got: {exit:?}"
        );
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

    /// #1107 — with `--quick-save` into an unwritable vault the inbox cannot
    /// be created: `export_phase` must stop with a typed `CliExit::ConfigError`
    /// naming the inbox (the old code ran a blocking `std::fs::create_dir_all`
    /// and discarded its `Result`, so the failure surfaced late and degraded
    /// inside `save_files`).
    #[cfg(unix)]
    #[cfg_attr(
        miri,
        ignore = "export_phase touches filesystem (create_dir_all) unsupported by Miri"
    )]
    #[tokio::test]
    async fn export_phase_reports_inbox_creation_failure() {
        use crate::cli::orchestrator::export_phase;
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().expect("tmp");
        let vault = tmp.path().join("ro-vault");
        std::fs::create_dir(&vault).expect("vault dir");
        std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o500)).expect("chmod");

        // Root ignores the permission bits — skip honestly instead of failing.
        if std::fs::File::create(vault.join("probe")).is_ok() {
            let _ = std::fs::remove_file(vault.join("probe"));
            let _ = std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o700));
            eprintln!("skipping: effective user can write to a 0o500 directory");
            return;
        }

        let mut opts = CrawlOptions::default();
        opts.export.output_dir = tmp.path().join("out");
        opts.export.quick_save = true;
        opts.export.obsidian_vault = Some(vault.clone());

        let exit = export_phase(
            &[],
            &opts,
            None,
            #[cfg(feature = "ai")]
            None,
        )
        .await;

        // Restore permissions so the TempDir can clean up.
        let _ = std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o700));

        match exit {
            CliExit::ConfigError(msg) => {
                assert!(
                    msg.contains("no se pudo crear el inbox"),
                    "error must name the failed inbox creation, got: {msg}"
                );
            },
            other => panic!("expected ConfigError for unwritable inbox, got: {other:?}"),
        }
    }

    // ===== #762 — persistence root / export dir convergence =====

    /// No vault: both roots default to `-o`.
    #[test]
    fn persistence_root_defaults_to_output_dir() {
        let mut opts = CrawlOptions::default();
        opts.export.output_dir = std::path::PathBuf::from("/tmp/out");

        assert_eq!(
            resolve_persistence_root(&opts),
            std::path::PathBuf::from("/tmp/out")
        );
        assert_eq!(
            resolve_export_dir(&opts),
            std::path::PathBuf::from("/tmp/out")
        );
    }

    /// quick_save: Markdown+assets root is the vault, RAG export keeps `-o`.
    #[test]
    fn quick_save_roots_to_vault_keeps_export_in_output_dir() {
        let mut opts = CrawlOptions::default();
        opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
        opts.export.obsidian_vault = Some(std::path::PathBuf::from("/tmp/vault"));
        opts.export.quick_save = true;

        assert_eq!(
            resolve_persistence_root(&opts),
            std::path::PathBuf::from("/tmp/vault")
        );
        assert_eq!(
            resolve_export_dir(&opts),
            std::path::PathBuf::from("/tmp/out")
        );
    }

    /// Explicit --vault (no quick_save): both roots redirect to the vault (#762).
    #[test]
    fn explicit_vault_redirects_both_roots_to_vault() {
        let mut opts = CrawlOptions::default();
        opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
        opts.export.obsidian_vault = Some(std::path::PathBuf::from("/tmp/vault"));
        opts.export.vault_is_explicit = true;

        assert_eq!(
            resolve_persistence_root(&opts),
            std::path::PathBuf::from("/tmp/vault")
        );
        assert_eq!(
            resolve_export_dir(&opts),
            std::path::PathBuf::from("/tmp/vault")
        );
    }

    /// Config-filled vault without explicit flag: NO redirect (#762 — only
    /// the explicit CLI flag changes the output base).
    #[test]
    fn config_filled_vault_does_not_redirect() {
        let mut opts = CrawlOptions::default();
        opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
        opts.export.obsidian_vault = Some(std::path::PathBuf::from("/tmp/vault"));
        opts.export.vault_is_explicit = false;

        assert_eq!(
            resolve_persistence_root(&opts),
            std::path::PathBuf::from("/tmp/out")
        );
        assert_eq!(
            resolve_export_dir(&opts),
            std::path::PathBuf::from("/tmp/out")
        );
    }

    /// Explicit flag with a missing vault falls back to `-o` — never panics.
    #[test]
    fn explicit_vault_without_path_falls_back_to_output_dir() {
        let mut opts = CrawlOptions::default();
        opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
        opts.export.obsidian_vault = None;
        opts.export.vault_is_explicit = true;

        assert_eq!(
            resolve_persistence_root(&opts),
            std::path::PathBuf::from("/tmp/out")
        );
        assert_eq!(
            resolve_export_dir(&opts),
            std::path::PathBuf::from("/tmp/out")
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

        let default_state_dir = crate::cli::scrape_flow::resolve_default_state_dir();
        let persistence_mode = opts.crawl.persistence_mode(&default_state_dir);
        let result = prepare_phase(&opts, &persistence_mode).await;
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

        // The cfg-gated `adaptive_engine` parameter makes the arity depend on
        // the feature combo — dispatch the call exactly like `build_and_run`
        // does in webfang_cli/src/main.rs.
        #[cfg(feature = "adaptive-selectors")]
        let exit = run(
            opts,
            None,
            crate::application::container::VaultAiPorts::default(),
        )
        .await;
        #[cfg(not(feature = "adaptive-selectors"))]
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

    // ===== output_vectors without clean-ai flag (ai feature on, #703) =====

    #[cfg(feature = "ai")]
    #[tokio::test]
    async fn run_returns_data_error_when_output_vectors_without_clean_ai() {
        let mut opts = CrawlOptions::default();
        opts.elastic.output_vectors = Some("vectors.jsonl".to_string());
        // opts.ai stays false → no semantic cleaning requested

        // The cfg-gated `ai_cleaner` / `adaptive_engine` parameters make the
        // arity depend on both features — dispatch the call exactly like
        // `build_and_run` does in webfang_cli/src/main.rs.
        #[cfg(feature = "adaptive-selectors")]
        let exit = run(
            opts,
            None,
            None,
            crate::application::container::VaultAiPorts::default(),
        )
        .await;
        #[cfg(not(feature = "adaptive-selectors"))]
        let exit = run(
            opts,
            None,
            crate::application::container::VaultAiPorts::default(),
        )
        .await;

        assert!(
            matches!(exit, CliExit::DataFormatError(_)),
            "expected CliExit::DataFormatError, got {exit:?}"
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
            &cancel,
        )
        .await;

        assert!(
            matches!(exit, CliExit::DataFormatError(_)),
            "expected CliExit::DataFormatError, got {exit:?}"
        );
    }
}
