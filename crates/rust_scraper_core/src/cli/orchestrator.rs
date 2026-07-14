//! CLI orchestrator — coordinates the main scraping pipeline.
//!
//! Orchestrates URL discovery, scraping, and export phases.

use tokio::task::JoinSet;
use tracing::{info, instrument, warn};

use crate::application::crawl_options::CrawlOptions;
use crate::cli::completions::generate_completions;
use crate::cli::error::CliExit;
use crate::cli::export_flow::{run_export, save_files, ExportConfig};
use crate::cli::scrape_flow::scrape_urls;
use crate::cli::url_discovery::discover_urls;
use crate::domain::repository::DynVectorRepository;
use crate::error::ScraperError;
use crate::Args;
use crate::CrawlerConfig;
use crate::ScraperConfig;

use crate::domain;
use crate::infrastructure::output::file_saver::ObsidianOptions;
use crate::Shell;

#[cfg(feature = "ai")]
use crate::domain::semantic_cleaner::SemanticCleaner;

/// Handle shell completion generation.
pub fn handle_completions(shell: Shell) -> CliExit {
    let clap_shell = match shell {
        Shell::Bash => clap_complete::Shell::Bash,
        Shell::Elvish => clap_complete::Shell::Elvish,
        Shell::Fish => clap_complete::Shell::Fish,
        Shell::PowerShell => clap_complete::Shell::PowerShell,
        Shell::Zsh => clap_complete::Shell::Zsh,
    };
    generate_completions::<Args>(clap_shell)
        .map(|_| CliExit::Success)
        .unwrap_or_else(|_| CliExit::UsageError("completion generation failed".into()))
}

/// Main orchestration entry point.
///
/// Coordinates the full scraping pipeline:
/// 1. URL discovery
/// 2. Scraping with progress
/// 3. Export results
#[instrument(level = "info", skip(opts, ai_cleaner), fields(url = %opts.url))]
pub async fn run(
    opts: CrawlOptions,
    #[cfg(feature = "ai")] ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
) -> CliExit {
    // Dry-run mode: list planned URLs without any network requests
    if opts.export.dry_run {
        println!("Dry-run: 1 URL(s) would be scraped:");
        println!("  {}", opts.url);
        return CliExit::Success;
    }

    // Batch mode: process URLs from stdin or file, then exit early
    if opts.batch.enabled {
        return run_batch(opts).await;
    }

    let urls_to_scrape = if opts.crawl.single_page {
        plan_urls(true, opts.url.clone(), Vec::new())
    } else {
        // Create crawler config from CrawlOptions
        let mut crawler_config = CrawlerConfig::builder(opts.url.clone())
            .max_pages(opts.crawl.max_pages)
            .max_depth(opts.crawl.max_depth)
            .include_patterns(opts.crawl.include_patterns.clone())
            .exclude_patterns(opts.crawl.exclude_patterns.clone())
            .ignore_robots(opts.crawl.ignore_robots)
            .use_sitemap(opts.crawl.use_sitemap);
        if let Some(ref sitemap_url) = opts.crawl.sitemap_url {
            crawler_config = crawler_config.sitemap_url(sitemap_url);
        }
        let crawler_config = crawler_config.build();

        // URL discovery phase
        let discovered_urls: Vec<url::Url> = discover_urls(&crawler_config, &opts).await;
        if discovered_urls.is_empty() {
            info!("No URLs discovered");
            return CliExit::Success;
        }

        plan_urls(false, opts.url.clone(), discovered_urls)
    };

    // Create scraper config
    let mut scraper_config = ScraperConfig::default()
        .with_output_dir(opts.export.output_dir.clone())
        .with_scraper_concurrency(opts.network.concurrency.resolve())
        .with_max_pages(opts.crawl.max_pages)
        .with_selector(opts.crawl.selector.clone());

    // Apply download flags (builder pattern requires conditional application)
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
    // Propagates error on failure — the user must know if asset downloads can't start.
    let shared_downloader = if scraper_config.has_downloads() {
        match crate::adapters::downloader::Downloader::new(scraper_config.to_download_config()) {
            Ok(dl) => Some(std::sync::Arc::new(dl)),
            Err(e) => {
                return CliExit::IoError(format!("No se pudo crear el descargador de assets: {e}"));
            },
        }
    } else {
        None
    };

    // Initialize elastic ingestion if requested (`--elastic` → SQLite, or
    // `--output-vectors` → headless JSONL stream). Both are erased to
    // `DynVectorRepository` so the field type is feature-independent.
    let elastic_ingestion = match build_elastic_ingestion(&opts).await {
        Ok(v) => v,
        Err(e) => return e,
    };

    // Scraping phase
    let (results, failures): (
        Vec<domain::ScrapedContent>,
        Vec<(String, crate::error::ScraperError)>,
    ) = scrape_urls(
        &urls_to_scrape,
        &scraper_config,
        &opts,
        None,
        shared_downloader.as_deref(),
    )
    .await;

    // Post-scrape: elastic ingestion. Fail-fast — a broken pipe / write error
    // (D2) propagates as a fatal `IoError` and aborts the crawl.
    if let Some(ref ingestion) = elastic_ingestion {
        if let Err(e) = run_elastic_ingestion(ingestion, &results).await {
            return CliExit::IoError(format!("Falló la ingesta de vectores: {e}"));
        }
    }

    // Report failures — preserve the full root-cause chain via `Error::source()`
    // so the cause (e.g. wreq::Error → I/O → timeout) is not flattened (D4).
    for (url, error) in &failures {
        let mut chain = error.to_string();
        let mut src = std::error::Error::source(error);
        while let Some(cause) = src {
            chain.push_str(&format!("  ← {cause}"));
            src = cause.source();
        }
        eprintln!("Failed to scrape {url}: {chain}");
    }

    if results.is_empty() {
        eprintln!("No pages were successfully scraped");
        return CliExit::NetworkError("No pages were successfully scraped".into());
    }

    info!("Successfully scraped {} pages", results.len());

    // Obsidian options
    let obsidian_options = ObsidianOptions {
        wiki_links: opts.export.obsidian_wiki_links,
        relative_assets: opts.export.obsidian_relative_assets,
        tags: opts.export.obsidian_tags.clone(),
        rich_metadata: opts.export.obsidian_rich_metadata,
        quick_save: opts.export.quick_save,
        vault_path: opts.export.obsidian_vault.clone(),
    };

    // Determine output directory for individual files
    let output_dir = if opts.export.quick_save {
        let base = opts
            .export
            .obsidian_vault
            .as_deref()
            .unwrap_or(&opts.export.output_dir);
        let inbox = base.join("_inbox");
        if !inbox.exists() {
            let _ = std::fs::create_dir_all(&inbox);
        }
        inbox
    } else {
        opts.export.output_dir.clone()
    };

    // Export phase
    let export_config = ExportConfig {
        results: &results,
        output_dir: opts.export.output_dir.clone(),
        format: opts.export.output_format,
        export_format: opts.export.export_format,
        clean_ai: opts.ai,
        quick_save: opts.export.quick_save,
        vault_path: opts.export.obsidian_vault.as_ref(),
        obsidian_options: obsidian_options.clone(),
        state_store: None, // TODO: Add state store
        resume: opts.crawl.resume,
        ai_threshold: 0.3, // TODO: Add AI settings from CrawlOptions
        ai_max_tokens: 512,
        ai_offline: false,
    };

    // Save individual files (Markdown, etc.)
    save_files(
        &results,
        &output_dir,
        &opts.export.output_format,
        &obsidian_options,
    );

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
            eprintln!("Export failed: {e:?}");
            e
        },
    }
}

/// Run the elastic ingestion pipeline on all scraped results.
///
/// Each URL is processed concurrently via a bounded `JoinSet` with
/// concurrency limited by the elastic config's CPU core count.
///
/// Fail-fast (frozen Decision 3 + D2): the first ingestion error — including a
/// broken pipe / `WriteZero` while streaming JSONL — propagates immediately and
/// aborts the crawl, rather than being swallowed as a warning.
async fn run_elastic_ingestion(
    ingestion: &std::sync::Arc<
        crate::application::elastic_ingestion::ElasticIngestion<DynVectorRepository>,
    >,
    results: &[crate::domain::ScrapedContent],
) -> Result<(), ScraperError> {
    if results.is_empty() {
        return Ok(());
    }

    let mut join_set = JoinSet::new();
    let concurrency = num_cpus::get().max(4); // bounded concurrency

    for result in results {
        let ing = std::sync::Arc::clone(ingestion);
        let url = result.url.clone();

        while join_set.len() >= concurrency {
            match join_set.join_next().await {
                // `T` is `Result<(), ScraperError>` (the spawned task's output).
                Some(Ok(Ok(()))) => {},            // success
                Some(Ok(Err(e))) => return Err(e), // ingestion error (D2 fail-fast)
                Some(Err(_join_err)) => {
                    return Err(ScraperError::ingestion(
                        "tarea de ingesta elástica cancelada",
                    ));
                },
                None => break,
            }
        }

        join_set.spawn(async move {
            let url_str = url.to_string();
            ing.run(&url_str).await
        });
    }

    // Await remaining tasks (propagate the first error — D2 fail-fast).
    while let Some(result) = join_set.join_next().await {
        if let Ok(Err(e)) = result {
            return Err(e);
        }
    }
    Ok(())
}

/// Build the elastic ingestion pipeline for the run.
///
/// - `persistence` ON + `--elastic` → SQLite-backed `SqliteVectorRepository`.
/// - `--output-vectors <path|->` → dependency-free `StreamRepository` JSONL sink
///   (available in every build, including the lightweight core binary).
/// - otherwise → `None` (no ingestion).
async fn build_elastic_ingestion(
    opts: &CrawlOptions,
) -> Result<
    Option<
        std::sync::Arc<
            crate::application::elastic_ingestion::ElasticIngestion<DynVectorRepository>,
        >,
    >,
    CliExit,
> {
    let container = match crate::application::container::Container::new(
        CrawlerConfig::new(opts.url.clone()),
        ScraperConfig::default(),
    )
    .await
    {
        Ok(c) => c,
        Err(e) => {
            if opts.elastic.enabled || opts.elastic.output_vectors.is_some() {
                return Err(CliExit::IoError(format!(
                    "no se pudo crear el contenedor para ingesta elástica: {e}"
                )));
            }
            warn!("no se pudo crear el contenedor para ingesta elástica: {e}");
            return Ok(None);
        },
    };

    let built = {
        #[cfg(feature = "persistence")]
        {
            if opts.elastic.enabled {
                container.with_elastic(opts).await
            } else if let Some(ref path) = opts.elastic.output_vectors {
                container.with_stream(opts, path)
            } else {
                Ok(container)
            }
        }
        #[cfg(not(feature = "persistence"))]
        {
            if let Some(ref path) = opts.elastic.output_vectors {
                container.with_stream(opts, path)
            } else {
                Ok(container)
            }
        }
    };

    match built {
        Ok(c) => Ok(c.elastic_ingestion),
        Err(e) => Err(CliExit::IoError(format!(
            "no se pudo inicializar la ingesta de vectores: {e}"
        ))),
    }
}

/// Run batch processing mode: crawl multiple URLs from stdin or file
async fn run_batch(opts: CrawlOptions) -> CliExit {
    use crate::application::batch::BatchManager;
    use crate::domain::CrawlerConfig;

    let mut crawler_config = CrawlerConfig::builder(opts.url.clone())
        .max_pages(opts.crawl.max_pages)
        .max_depth(opts.crawl.max_depth)
        .include_patterns(opts.crawl.include_patterns.clone())
        .exclude_patterns(opts.crawl.exclude_patterns.clone())
        .ignore_robots(opts.crawl.ignore_robots)
        .use_sitemap(opts.crawl.use_sitemap);
    if let Some(ref sitemap_url) = opts.crawl.sitemap_url {
        crawler_config = crawler_config.sitemap_url(sitemap_url);
    }
    let crawler_config = crawler_config.build();

    let manager_result = if let Some(ref path) = opts.batch.batch_file {
        info!("Reading URLs from file: {}", path.display());
        BatchManager::from_file(path, crawler_config, opts.batch.concurrency)
    } else {
        info!("Reading URLs from stdin");
        BatchManager::from_stdin(crawler_config, opts.batch.concurrency)
    };

    let manager = match manager_result {
        Ok(m) => m,
        Err(e) => {
            eprintln!("Failed to read URLs: {e}");
            return CliExit::NetworkError(format!("Failed to read URLs: {e}"));
        },
    };

    if manager.job_count() == 0 {
        eprintln!("No URLs provided for batch processing");
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
        eprintln!("  Failed: {url} — {err}");
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
    seed_url: url::Url,
    discovered_urls: Vec<url::Url>,
) -> Vec<url::Url> {
    if single_page {
        vec![seed_url]
    } else {
        discovered_urls
    }
}

/// Parse asset naming strategy from CLI string.
fn parse_asset_naming(s: &str) -> crate::adapters::downloader::AssetNamingStrategy {
    use crate::adapters::downloader::AssetNamingStrategy;
    match s.to_lowercase().as_str() {
        "slug" => AssetNamingStrategy::Slug,
        "content-disposition" => AssetNamingStrategy::ContentDisposition,
        _ => AssetNamingStrategy::Hash,
    }
}

/// Parse H2/TLS profile from CLI string.
///
/// Resolves a profile name string to a `wreq_util::Profile` variant.
///
/// Tries exact match against known variants; defaults to `Chrome145` on
/// unknown input.  This intentionally covers a subset of the ~100+
/// variants — users who need edge/okhttp/safari profiles can configure
/// the H2 profile via the HTTP client config directly.
fn parse_asset_h2_profile(s: &str) -> wreq_util::Profile {
    use wreq_util::Profile;

    match s {
        // Chrome
        "Chrome100" => Profile::Chrome100,
        "Chrome101" => Profile::Chrome101,
        "Chrome104" => Profile::Chrome104,
        "Chrome107" => Profile::Chrome107,
        "Chrome110" => Profile::Chrome110,
        "Chrome116" => Profile::Chrome116,
        "Chrome120" => Profile::Chrome120,
        "Chrome131" => Profile::Chrome131,
        "Chrome145" => Profile::Chrome145,
        // Firefox
        "Firefox135" => Profile::Firefox135,
        "FirefoxAndroid135" => Profile::FirefoxAndroid135,
        // Safari
        "Safari18" => Profile::Safari18,
        "SafariIos18_1_1" => Profile::SafariIos18_1_1,
        "SafariIPad18" => Profile::SafariIPad18,
        // OkHttp
        "OkHttp4_12" => Profile::OkHttp4_12,
        "OkHttp5" => Profile::OkHttp5,
        // Fallback
        _ => {
            tracing::warn!(
                "Unknown asset H2 profile '{s}', falling back to Chrome145. \
                 Run `cargo doc -p wreq-util` to see all available profiles."
            );
            Profile::Chrome145
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{batch_exit_code, plan_urls};
    use crate::cli::error::CliExit;

    #[test]
    fn plan_urls_returns_only_seed_url_for_single_page() {
        let seed_url = url::Url::parse("https://example.com").expect("valid seed url");
        let discovered_urls = vec![
            url::Url::parse("https://example.com/about").expect("valid discovered url"),
            url::Url::parse("https://example.com/blog").expect("valid discovered url"),
        ];

        let planned = plan_urls(true, seed_url.clone(), discovered_urls);

        assert_eq!(planned, vec![seed_url]);
    }

    #[test]
    fn plan_urls_keeps_discovered_urls_in_normal_mode() {
        let seed_url = url::Url::parse("https://example.com").expect("valid seed url");
        let discovered_urls = vec![
            url::Url::parse("https://example.com/about").expect("valid discovered url"),
            url::Url::parse("https://example.com/blog").expect("valid discovered url"),
        ];

        let planned = plan_urls(false, seed_url, discovered_urls.clone());

        assert_eq!(planned, discovered_urls);
    }

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
}
