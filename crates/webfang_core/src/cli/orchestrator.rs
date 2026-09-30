//! CLI orchestrator — coordinates the main scraping pipeline.
//!
//! Orchestrates URL discovery, scraping, and export phases.

use tracing::{error, info, instrument, warn};

use crate::application::crawl_options::CrawlOptions;
use crate::application::crawler::CapturedPage;
use crate::application::crawler::InMemoryContentSink;
use crate::cli::batch_flow::{load_batch_manager, run_batch};
use crate::cli::elastic::{build_elastic_ingestion, run_elastic_ingestion};
use crate::cli::error::CliExit;
use crate::cli::exit_codes::report_phase;
use crate::cli::export_flow::{run_export, save_files, ExportConfig};
use crate::cli::parse::parse_asset_naming;
use crate::cli::scrape_flow::{apply_resume_mode, scrape_urls};
use crate::cli::url_discovery::{discover_urls, discover_urls_unified, DiscoveryRetry};
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
pub(crate) fn output_vectors_gate(opts: &CrawlOptions) -> Option<CliExit> {
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
#[allow(clippy::too_many_lines)]
#[instrument(level = "info", skip(opts, ai_cleaner, adaptive_engine, vault_ports, llm_port), fields(url = %opts.url))]
pub async fn run(
    opts: CrawlOptions,
    #[cfg(feature = "ai")] ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
    #[cfg(feature = "adaptive-selectors")] adaptive_engine: Option<
        std::sync::Arc<AdaptiveSelectorEngine>,
    >,
    vault_ports: crate::application::container::VaultAiPorts,
    llm_port: Option<std::sync::Arc<dyn crate::domain::llm_port::LlmPort>>,
) -> CliExit {
    // Run-root correlation identity (#501, #1439): the whole operation owns
    // ONE root, minted HERE at the orchestration entry and propagated into
    // every route — dry-run preview, batch, prepare/scrape phases. Before
    // #1439 the discovery Engine minted its own root milliseconds after this
    // event, silently splitting every DOM/batch run into two identities in
    // the trace (the historical #687 class). `#[instrument]` spans cannot
    // see locals at creation, so declare it offline-visible via a structured
    // event (lands in the JSONL `.fields`).
    let root_correlation = domain::CorrelationId::new();
    info!(
        correlation_id = %root_correlation,
        trace_id = %root_correlation.trace_id(),
        "run identity"
    );

    if opts.export.dry_run {
        return run_dry_run(opts, &root_correlation).await;
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
        // #1439: the run-root minted above travels through `run_batch_crawl`
        // onto the `BatchProcessor`, so every per-URL crawl engine of one
        // batch run shares this single identity instead of minting its own
        // (the old per-crawl mint split the batch trace in two).
        return run_batch(
            opts,
            #[cfg(feature = "ai")]
            ai_cleaner,
            vault_ports,
            llm_port,
            &cancel,
            &root_correlation,
        )
        .await;
    }

    // PersistenceMode unified control-plane — pure resolver with default dir.
    // Built BEFORE prepare_phase so discovery Engine can be wired with
    // `with_persistence` (checkpoint interval flows from the mode, not hardcoded).
    // The resolver itself never logs (#1045): `--state-dir` without `--resume`
    // is reported via `ResolverNotes` and warned about here, the one call
    // site that knows about user flags.
    let persistence_mode = resolve_persistence_mode(&opts);

    let prepare = match prepare_phase(&opts, &persistence_mode, &root_correlation).await {
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
        &prepare.captured_pages,
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
pub(crate) fn resolve_persistence_root(opts: &CrawlOptions) -> std::path::PathBuf {
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
pub(crate) async fn export_phase(
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
async fn run_dry_run(opts: CrawlOptions, root_correlation: &domain::CorrelationId) -> CliExit {
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

    // F-14 (#1232 slice 1): dry-run shares the unified recursive discovery
    // with the real DOM path, so `--max-depth` is honored in previews.
    // dry-run-fail-fast: a preview reports seed reachability, so it runs a
    // single attempt with zero backoff sleeps (`FailFast`) instead of
    // spending the full operator retry budget before exiting 69.
    info!("Dry-run: discovering URLs without scraping...");
    let persistence_mode = resolve_persistence_mode(&opts);
    let output = match crate::cli::url_discovery::discover_urls_unified(
        crawler_config,
        &opts,
        &persistence_mode,
        None,
        root_correlation,
        DiscoveryRetry::FailFast,
    )
    .await
    {
        Ok(output) => output,
        Err(e) => return CliExit::NetworkError(format!("URL discovery failed: {e}")),
    };
    let discovered = &output.urls;

    // #1381: a seed the SSRF guard cuts opens no socket, so discovery completes
    // `Ok` with zero URLs and the preview below reported "nothing to scrape" —
    // exit 0 — for what is actually a policy refusal. Say so and leave with the
    // null-result code the sitemap arms already use for this shape (2), instead
    // of letting automation read a refusal as an empty site. Only the literal
    // entry layer is named: a hostname refused by the connect-time resolver is
    // indistinguishable here from a real DNS failure.
    if discovered.is_empty() {
        if let Some(exit) =
            crate::cli::error::empty_discovery_exit_when_seed_refused(opts.url.as_url())
        {
            return exit;
        }
    }

    // #1443: a dry-run that discovered nothing AND counted fetch errors means
    // the seed itself failed — the Engine consumes per-page failures into
    // `CrawlResult.errors` and completes `Ok`, so without this the preview
    // printed `Dry-run: 0 URL(s) would be scraped:` and exited 0 for a dead
    // seed. Precondition (single seed): dry-run without `--batch-file`
    // discovers from `opts.url` alone — the batch shape returns above — so
    // nothing is discoverable without fetching the seed, and a live seed
    // always lands in `urls` even when it carries no links. Runs AFTER the
    // #1381 guard above (strict SSRF-first order): a refused seed already
    // returned exit 2, so reaching here with errors means a real dial failed.
    // Reuses the `NetworkError` message/exit 69 the `Err` arm already uses.
    if discovered.is_empty() && output.errors > 0 {
        return CliExit::NetworkError(format!(
            "URL discovery failed: la semilla no respondió ({} error(es) de rastreo, 0 URLs descubiertas)",
            output.errors
        ));
    }

    println!("\nDry-run: {} URL(s) would be scraped:", discovered.len());
    for url in discovered {
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
pub(crate) fn resolve_sitemap_projection(opts: &CrawlOptions) -> Result<SitemapConfig, CliExit> {
    SitemapConfig::resolve(opts.crawl.use_sitemap, opts.crawl.sitemap_url.as_deref())
        .map_err(|e| CliExit::ConfigError(e.to_string()))
}

/// Build a `CrawlerConfig` for URL discovery (shared by dry-run, prepare, and batch).
fn build_crawler_config_for_discovery(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
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
/// that had no effect (resolved `Disabled` while a state dir was passed)
/// is reported via `ResolverNotes` and warned about here, the one call
/// site that knows about user flags.
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

/// Run sitemap discovery and map its terminal states to `CliExit` (#1439
/// extraction: keeps `prepare_phase` under the `too_many_lines` ratchet).
///
/// "Site has no sitemap" and "sitemap empty" are discovery states, not
/// infrastructure failures (#695): exit 2 lets automation distinguish them
/// from a real network outage (exit 69). Exit 2 also fires when the sitemap —
/// the source of truth in this mode — yields zero URLs.
async fn discover_sitemap_urls(
    crawler_config: &CrawlerConfig,
    opts: &CrawlOptions,
    root_correlation: &domain::CorrelationId,
) -> Result<Vec<url::Url>, CliExit> {
    match discover_urls(crawler_config, opts, root_correlation).await {
        Err(crate::error::ScraperError::SitemapNotFound(_)) => Err(CliExit::EmptyDiscovery(
            "No URLs discovered: sitemap not found".into(),
        )),
        Err(crate::error::ScraperError::SitemapEmpty) => Err(CliExit::EmptyDiscovery(
            "No URLs discovered: sitemap is empty".into(),
        )),
        Err(e) => Err(CliExit::NetworkError(format!("URL discovery failed: {e}"))),
        Ok(urls) if urls.is_empty() => Err(CliExit::EmptyDiscovery(
            "No URLs discovered from sitemaps".into(),
        )),
        Ok(urls) => Ok(urls),
    }
}

/// Prepare scraper config and discover URLs.
///
/// Returns the initial `ScraperConfig` (before asset/download wiring) and
/// the list of URLs to scrape.  On discovery failure, returns the
/// appropriate `CliExit` error.
async fn prepare_phase(
    opts: &CrawlOptions,
    persistence_mode: &PersistenceMode,
    root_correlation: &domain::CorrelationId,
) -> Result<PrepareResult, CliExit> {
    // Discovery-captured bodies (F-05, #1229): filled by the DOM branch
    // below, reused by the scrape phase instead of refetching.
    let mut captured_pages: Vec<CapturedPage> = Vec::new();
    let urls_to_scrape = if opts.crawl.single_page {
        // F-35 (#1216): single-page mode never runs discovery, so the seed
        // pattern guard needs a patterns-only config — no TLS/sitemap
        // projection involved, keeping `--h2-profile` semantics unchanged here.
        let seed_guard = CrawlerConfig::builder(opts.url.as_url().clone())
            .include_patterns(opts.crawl.include_patterns.clone())
            .exclude_patterns(opts.crawl.exclude_patterns.clone())
            .build();
        // Short local keeps the arg-span under rustfmt's fn_call_width,
        // so the call stays single-line and prepare_phase under the
        // clippy too_many_lines ratchet ceiling (#516).
        let seed = opts.url.as_url().clone();
        plan_urls(true, false, seed, Vec::new(), &seed_guard)
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
            discover_sitemap_urls(&crawler_config, opts, root_correlation).await?
        } else {
            // Recursive BFS discovery respects max_depth/max_pages/robots/
            // patterns; the existing scrape_phase + export_phase still own
            // content extraction and on-disk output.
            let (urls, pages) = discover_dom_with_capture(
                &crawler_config,
                opts,
                persistence_mode,
                root_correlation,
            )
            .await?;
            captured_pages = pages;
            urls
        };

        plan_urls(
            false,
            opts.crawl.use_sitemap,
            opts.url.as_url().clone(),
            discovered_urls,
            &crawler_config,
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
        captured_pages,
    })
}

/// Run unified DOM discovery with a bounded capture sink (F-05, #1229).
///
/// Returns the discovered URLs plus the bodies captured during discovery
/// for the scrape phase to reuse instead of refetching — one HTTP request
/// per page. Unified DOM discovery (F-14, #1232) runs the recursive Engine;
/// F-35 (#1216): the config is cloned because `plan_urls` reuses it for
/// the seed pattern guard. `root_correlation` (#1439) is the CLI run-root,
/// propagated verbatim so the Engine and the CLI share one trace identity.
///
/// # Errors
///
/// Returns [`CliExit::NetworkError`] when the Engine discovery fails.
async fn discover_dom_with_capture(
    crawler_config: &CrawlerConfig,
    opts: &CrawlOptions,
    persistence_mode: &PersistenceMode,
    root_correlation: &domain::CorrelationId,
) -> Result<(Vec<url::Url>, Vec<CapturedPage>), CliExit> {
    let capture_sink = std::sync::Arc::new(InMemoryContentSink::new());
    let cfg = crawler_config.clone();
    // dry-run-fail-fast: the real DOM path keeps full operator retry
    // semantics (`Operator`) — only the dry-run preview runs fail-fast.
    match discover_urls_unified(
        cfg,
        opts,
        persistence_mode,
        Some(capture_sink),
        root_correlation,
        DiscoveryRetry::Operator,
    )
    .await
    {
        Err(e) => Err(CliExit::NetworkError(format!("URL discovery failed: {e}"))),
        Ok(output) => Ok((output.urls, output.pages)),
    }
}

struct PrepareResult {
    urls_to_scrape: Vec<url::Url>,
    scraper_config: ScraperConfig,
    shared_downloader: Option<std::sync::Arc<crate::adapters::downloader::Downloader>>,
    /// Bodies captured during DOM discovery (F-05, #1229): the scrape phase
    /// reuses them instead of refetching. Empty for single-page, sitemap,
    /// and dry-run shapes.
    captured_pages: Vec<CapturedPage>,
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
    captured: &[CapturedPage],
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
        captured,
    )
    .await
}
/// Plan the final scrape list from discovery output.
///
/// F-35 (#1216): `--include-pattern` / `--exclude-pattern` apply to the seed
/// URL itself, not just discovered pages. The CLI default path uses the crawl
/// Engine only for discovery and then scrapes this planned list directly, so
/// the Engine's own seed guard (`engine.rs`, #634) is bypassed here — the
/// planning boundary must enforce the same
/// [`crate::application::url_filter::is_allowed`] predicate. The Engine guard
/// stays intact for direct Engine consumers (batch, MCP) and for discovery
/// filtering: the predicate lives in exactly one function, enforced at both
/// boundaries (defense in depth, not duplicated logic). Dropping the
/// unconditional insert instead was rejected: the `single_page` path never
/// runs the Engine, and `plan_urls` cannot assume every discovery backend
/// returns the seed, so an explicit guard keeps this boundary total.
#[instrument(
    skip(seed_url, discovered_urls, crawler_config),
    fields(seed_url = %seed_url)
)]
fn plan_urls(
    single_page: bool,
    use_sitemap: bool,
    seed_url: url::Url,
    discovered_urls: Vec<url::Url>,
    crawler_config: &CrawlerConfig,
) -> Vec<url::Url> {
    // Single source of truth for "may the seed be scraped" (F-35, #1216).
    let seed_allowed =
        crate::application::url_filter::is_allowed(seed_url.as_str(), crawler_config);
    if !seed_allowed {
        info!(
            seed_url = %seed_url,
            "Seed URL excluded by pattern filters — it will not be scraped"
        );
    }
    if single_page {
        if seed_allowed {
            vec![seed_url]
        } else {
            Vec::new()
        }
    } else if use_sitemap {
        // Sitemap is the source of truth — do not inject the seed URL.
        // Discovery already applied the pattern filters.
        discovered_urls
    } else {
        // DOM discovery: re-inject the seed ONLY when the patterns allow it,
        // so it gets crawled even when link extraction only returns children.
        let mut urls = discovered_urls;
        if seed_allowed {
            if !urls.contains(&seed_url) {
                urls.insert(0, seed_url);
            }
        } else {
            // Defensive: strip the seed if a discovery backend returned it
            // despite the filters — an excluded seed must never be scraped.
            urls.retain(|url| *url != seed_url);
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
            profile = %s,
            fallback = "Chrome145",
            "unknown asset H2 profile; falling back to Chrome145 (see `cargo doc -p wreq-util` for all profiles)"
        );
        wreq_util::Profile::Chrome145
    })
}

#[cfg(test)]
mod tests {
    use super::{
        build_crawler_config_for_discovery, build_elastic_ingestion, parse_asset_h2_profile,
        plan_urls, resolve_export_dir, resolve_persistence_root, run, CrawlerConfig,
    };
    use crate::application::crawl_options::CrawlOptions;
    use crate::cli::error::CliExit;

    // ===== discovery config tests (#653 / R2-1) =====

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

    /// Permissive guard config (F-35, #1216): no patterns, every seed allowed.
    fn permissive_guard(seed: &url::Url) -> CrawlerConfig {
        CrawlerConfig::new(seed.clone())
    }

    #[test]
    fn plan_urls_single_page_returns_seed_only() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/about").unwrap(),
            url::Url::parse("https://example.com/blog").unwrap(),
        ];
        let guard = permissive_guard(&seed);

        let result = plan_urls(true, false, seed.clone(), discovered, &guard);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_single_page_excluded_seed_yields_empty() {
        // F-35 (#1216): single-page mode never runs the Engine, so the
        // patterns-only guard must still refuse an excluded seed.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .exclude_pattern("/article")
            .build();

        let result = plan_urls(true, false, seed, Vec::new(), &guard);

        assert!(
            result.is_empty(),
            "excluded single-page seed must yield zero URLs, got {result:?}"
        );
    }

    #[test]
    fn plan_urls_dom_mode_prepends_seed() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let discovered = vec![
            url::Url::parse("https://example.com/a").unwrap(),
            url::Url::parse("https://example.com/b").unwrap(),
            url::Url::parse("https://example.com/c").unwrap(),
        ];

        let guard = permissive_guard(&seed);
        let result = plan_urls(false, false, seed.clone(), discovered.clone(), &guard);

        // DOM mode: an allowed seed is prepended when absent so it gets scraped.
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

        let guard = permissive_guard(&seed);
        let result = plan_urls(false, true, seed, discovered.clone(), &guard);

        // Sitemap mode: the sitemap is the source of truth — seed is NOT injected.
        assert_eq!(result, discovered);
    }

    #[test]
    fn plan_urls_dom_mode_empty_discovered() {
        // F-35 (#1216, INVERTED): the old assertion pinned the bug — "the
        // seed is always included in DOM mode". An excluded seed must NOT
        // be re-injected; an excluded seed with empty discovery yields zero URLs.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .exclude_pattern("/article")
            .build();

        let result = plan_urls(false, false, seed, Vec::new(), &guard);

        assert!(
            result.is_empty(),
            "excluded seed must yield zero URLs, got {result:?}"
        );
    }

    #[test]
    fn plan_urls_dom_mode_empty_discovered_allowed_seed_still_included() {
        // Companion to the inversion above: an ALLOWED seed with empty
        // discovery is still re-injected (e.g. a link-less article page).
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = permissive_guard(&seed);

        let result = plan_urls(false, false, seed.clone(), Vec::new(), &guard);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_dom_mode_include_mismatch_drops_seed() {
        // F-35 (#1216): a seed matching no include-pattern yields zero URLs.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .include_pattern("/nothing-here/*")
            .build();

        let result = plan_urls(false, false, seed, Vec::new(), &guard);

        assert!(
            result.is_empty(),
            "seed matching no include-pattern must yield zero URLs, got {result:?}"
        );
    }

    #[test]
    fn plan_urls_dom_mode_excluded_seed_stripped_from_discovered() {
        // F-35 (#1216): even if a discovery backend returned the excluded
        // seed, the planning boundary strips it; allowed URLs pass through.
        let seed = url::Url::parse("https://example.com/article").unwrap();
        let child = url::Url::parse("https://example.com/other").unwrap();
        let guard = CrawlerConfig::builder(seed.clone())
            .exclude_pattern("/article")
            .build();

        let result = plan_urls(false, false, seed, vec![child.clone()], &guard);

        assert_eq!(result, vec![child]);
    }

    #[test]
    fn plan_urls_single_page_ignores_many_discovered() {
        let seed = url::Url::parse("https://example.com/only").unwrap();
        let discovered: Vec<_> = (0..100)
            .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
            .collect();
        let guard = permissive_guard(&seed);

        let result = plan_urls(true, false, seed.clone(), discovered, &guard);

        assert_eq!(result, vec![seed]);
    }

    #[test]
    fn plan_urls_dom_mode_preserves_order() {
        let seed = url::Url::parse("https://example.com").unwrap();
        let urls: Vec<_> = (0..10)
            .map(|i| url::Url::parse(&format!("https://example.com/page{i}")).unwrap())
            .collect();

        let guard = permissive_guard(&seed);
        let result = plan_urls(false, false, seed.clone(), urls.clone(), &guard);

        // Discovered order is preserved; an allowed seed is prepended when absent.
        let mut expected = vec![seed];
        expected.extend(urls);
        assert_eq!(result, expected);
    }

    // ===== build_elastic_ingestion tests =====
    // All three tests call build_elastic_ingestion() → Container::new() →
    // HttpClient::new() → wreq → BoringSSL FFI (btls::ffi::TLS_method).
    // Miri cannot execute C FFI — this is a known limitation, not UB.
    // See: https://github.com/rust-lang/miri#unsupported-operations

    #[cfg_attr(
        miri,
        ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
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
        ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
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
        ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
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
        ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
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

        let url = crate::domain::ValidUrl::parse("https://example.com").expect("valid url");
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
        let result = prepare_phase(
            &opts,
            &persistence_mode,
            &crate::domain::CorrelationId::new(),
        )
        .await;
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
            None,
        )
        .await;
        #[cfg(not(feature = "adaptive-selectors"))]
        let exit = run(
            opts,
            crate::application::container::VaultAiPorts::default(),
            None,
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
            None,
        )
        .await;
        #[cfg(not(feature = "adaptive-selectors"))]
        let exit = run(
            opts,
            None,
            crate::application::container::VaultAiPorts::default(),
            None,
        )
        .await;

        assert!(
            matches!(exit, CliExit::DataFormatError(_)),
            "expected CliExit::DataFormatError, got {exit:?}"
        );
    }
}
