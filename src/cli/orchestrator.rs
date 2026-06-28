//! CLI orchestrator — coordinates the main scraping pipeline.
//!
//! Orchestrates URL discovery, scraping, and export phases.

use std::sync::Arc;

use tokio::task::JoinSet;
use tracing::{info, warn};

use crate::cli::completions::generate_completions;
use crate::cli::error::CliExit;
use crate::cli::export_flow::{run_export, save_files, ExportConfig};
use crate::cli::scrape_flow::scrape_urls;
use crate::cli::url_discovery::discover_urls;
use crate::Args;
use crate::CrawlerConfig;
use crate::ScraperConfig;

use crate::domain;
use crate::infrastructure::output::file_saver::ObsidianOptions;
use crate::Shell;

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
pub async fn run(args: Args) -> CliExit {
    let target_url_str = match args.url.as_ref() {
        Some(url) => url,
        None => return CliExit::UsageError("--url is required".into()),
    };

    let target_url = match url::Url::parse(target_url_str) {
        Ok(url) => url,
        Err(e) => return CliExit::UsageError(format!("Invalid URL: {e}")),
    };

    // Create crawler config from args
    let crawler_config = CrawlerConfig::builder(target_url.clone())
        .max_pages(args.max_pages)
        .max_depth(args.max_depth)
        .include_patterns(args.include_patterns.clone())
        .exclude_patterns(args.exclude_patterns.clone())
        .build();

    let urls_to_scrape = if args.single_page {
        plan_urls(true, target_url.clone(), Vec::new())
    } else {
        // URL discovery phase
        let discovered_urls: Vec<url::Url> = discover_urls(&crawler_config, &args).await;
        if discovered_urls.is_empty() {
            info!("No URLs discovered");
            return CliExit::Success;
        }

        plan_urls(false, target_url.clone(), discovered_urls)
    };

    // Create scraper config
    let mut scraper_config = ScraperConfig::default()
        .with_output_dir(args.output.clone())
        .with_scraper_concurrency(args.concurrency.resolve())
        .with_max_pages(args.max_pages);

    // Apply download flags (builder pattern requires conditional application)
    if args.download_images {
        scraper_config = scraper_config.with_images();
    }
    if args.download_documents {
        scraper_config = scraper_config.with_documents();
    }

    // Initialize elastic ingestion if requested
    let elastic: Option<
        Arc<
            crate::application::elastic_ingestion::ElasticIngestion<
                crate::infrastructure::persistence::sqlite::SqliteVectorRepository,
            >,
        >,
    > = if args.elastic {
        match build_elastic_pipeline(&args).await {
            Ok(ingestion) => {
                info!(
                    "pipeline elástico activado: db={}",
                    args.db_path
                        .as_deref()
                        .unwrap_or(std::path::Path::new("elastic.db"))
                        .display()
                );
                Some(ingestion)
            },
            Err(e) => {
                warn!("no se pudo inicializar el pipeline elástico: {e}");
                None
            },
        }
    } else {
        None
    };

    // Scraping phase

    let (results, failures): (Vec<domain::ScrapedContent>, Vec<(String, String)>) =
        scrape_urls(&urls_to_scrape, &scraper_config, &args, None).await;

    // Post-scrape: elastic ingestion (best-effort, no abort on failure)
    if let Some(ref ingestion) = elastic {
        run_elastic_ingestion(ingestion, &results).await;
    }

    // Report failures
    for (url, error) in &failures {
        eprintln!("Failed to scrape {url}: {error}");
    }

    if results.is_empty() {
        eprintln!("No pages were successfully scraped");
        return CliExit::NetworkError("No pages were successfully scraped".into());
    }

    info!("Successfully scraped {} pages", results.len());

    // Obsidian options
    let obsidian_options = ObsidianOptions {
        wiki_links: args.obsidian_wiki_links,
        relative_assets: args.obsidian_relative_assets,
        tags: args.obsidian_tags.clone().unwrap_or_default(),
        rich_metadata: args.obsidian_rich_metadata,
        quick_save: args.quick_save,
        vault_path: args.vault.clone(),
    };

    // Determine output directory for individual files
    let output_dir = if args.quick_save {
        if let Some(v) = &args.vault {
            let inbox = v.join("_inbox");
            if !inbox.exists() {
                let _ = std::fs::create_dir_all(&inbox);
            }
            inbox
        } else {
            args.output.clone()
        }
    } else {
        args.output.clone()
    };

    // Export phase
    let export_config = ExportConfig {
        results: &results,
        output_dir: args.output.clone(), // RAG export still goes to output_dir
        format: args.format,
        export_format: args.export_format,
        clean_ai: args.clean_ai,
        quick_save: args.quick_save,
        vault_path: args.vault.as_ref(),
        obsidian_options: obsidian_options.clone(),
        state_store: None, // TODO: Add state store
        resume: args.resume,
        ai_threshold: 0.3, // TODO: Add AI settings from args
        ai_max_tokens: 512,
        ai_offline: false,
    };

    // Save individual files (Markdown, etc.)
    save_files(&results, &output_dir, &args.format, &obsidian_options);

    match run_export(export_config).await {
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

/// Build the elastic ingestion pipeline from CLI args.
///
/// Wires `RayonCpuPool` → `CpuBridge` → `SqliteVectorRepository` →
/// `ResourceDownloader` → `ElasticIngestion<SqliteVectorRepository>`
/// using `ElasticConfig::resolve` with the CLI-provided overrides.
async fn build_elastic_pipeline(
    args: &Args,
) -> Result<
    Arc<
        crate::application::elastic_ingestion::ElasticIngestion<
            crate::infrastructure::persistence::sqlite::SqliteVectorRepository,
        >,
    >,
    Box<dyn std::error::Error + Send + Sync>,
> {
    use crate::application::elastic_ingestion::ElasticIngestion;
    use crate::infrastructure::autotuning::ElasticConfig;
    use crate::infrastructure::bridge::CpuBridge;
    use crate::infrastructure::config::AutotuningConfig;
    use crate::infrastructure::cpu_pool::RayonCpuPool;
    use crate::infrastructure::crawler::resource_downloader::ResourceDownloader;
    use crate::infrastructure::persistence::sqlite::{
        self as sqlite_persistence, SqliteVectorRepository,
    };

    let config = ElasticConfig::resolve(&args.elastic_overrides());

    let cpu_pool = RayonCpuPool::new(config.cpu_cores)?;
    let bridge = CpuBridge::new(cpu_pool);

    let pool = sqlite_persistence::create_pool(&config.db_path, config.db_pool_size)?;
    sqlite_persistence::setup_schema(&pool).await?;
    let repository = SqliteVectorRepository::new(pool);

    let client = crate::application::http_client::create_http_client()?;
    let max_concurrent = (config.ram_budget_bytes / config.max_resource_bytes).max(1) as usize;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
    let downloader = ResourceDownloader::with_config(
        semaphore,
        client,
        crate::infrastructure::crawler::resource_downloader::DownloadConfig {
            max_size_bytes: config.max_resource_bytes,
            ..Default::default()
        },
    );

    let autotune = AutotuningConfig::from_elastic(&config);
    let ingestion = ElasticIngestion::new(downloader, bridge, repository, autotune);

    Ok(Arc::new(ingestion))
}

/// Run the elastic ingestion pipeline on all scraped results.
///
/// Each URL is processed concurrently via a bounded `JoinSet` with
/// concurrency limited by the elastic config's CPU core count.
/// Ingestion failures are logged but do NOT abort the export phase
/// (best-effort semantics).
async fn run_elastic_ingestion(
    ingestion: &Arc<
        crate::application::elastic_ingestion::ElasticIngestion<
            crate::infrastructure::persistence::sqlite::SqliteVectorRepository,
        >,
    >,
    results: &[crate::domain::ScrapedContent],
) {
    if results.is_empty() {
        return;
    }

    let mut join_set = JoinSet::new();
    let concurrency = num_cpus::get().max(4); // bounded concurrency

    for result in results {
        let ing = Arc::clone(ingestion);
        let url = result.url.clone();

        while join_set.len() >= concurrency {
            match join_set.join_next().await {
                Some(Ok(Ok(()))) => {}, // success
                Some(Ok(Err(e))) => warn!("error en tarea de ingesta elástica: {e}"),
                Some(Err(e)) => warn!("error en tarea de ingesta elástica: {e}"),
                None => break,
            }
        }

        join_set.spawn(async move {
            let url_str = url.to_string();
            ing.run(&url_str).await
        });
    }

    // Await remaining tasks (all result variants, not just panics)
    while let Some(result) = join_set.join_next().await {
        match result {
            Ok(Ok(())) => {}, // success
            Ok(Err(e)) => warn!("error en tarea de ingesta elástica: {e}"),
            Err(e) => warn!("error en tarea de ingesta elástica: {e}"),
        }
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

#[cfg(test)]
mod tests {
    use super::plan_urls;

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
}
