//! Main orchestrator — coordinates the full scraping pipeline.
//!
//! The orchestrator handles URL discovery, TUI selection, scraping, and export.
//! It delegates config merging to `preflight` and export logic to `export_flow`.

use std::time::Instant;
use tracing::{info, warn};
use rust_scraper::cli::error::{format_cli_error, CliError, CliExit};
use rust_scraper::cli::summary::ScrapeSummary;
use rust_scraper::{
    get_random_user_agent_from_pool, Args, CrawlerConfig,
    ScraperConfig, UserAgentCache,
};



use rust_scraper::cli::commands::{preflight, PreflightContext};
use rust_scraper::cli::SelectedUrls;
use rust_scraper::cli::url_discovery::{discover_urls, select_urls};
use rust_scraper::cli::scrape_flow::{apply_resume_mode, scrape_urls};
use rust_scraper::cli::export_flow::{run_export_flow, determine_output_dir, build_obsidian_options};
use rust_scraper::export_flow;



/// Run the full scraping pipeline.
///
/// This function orchestrates:
/// 1. Vault detection
/// 2. User agent loading
/// 3. URL validation and pre-flight check
/// 4. URL discovery
/// 5. TUI selection or headless mode
/// 6. Resume filtering
/// 7. Scraping loop
/// 8. Export and file saving
/// 9. Summary printing
pub async fn run(args: Args) -> CliExit {
    // Target URL is guaranteed to exist (checked by caller)
    let target_url = args.url.clone().expect("url required");

    // Run preflight checks
    let PreflightContext {
        vault_path: _,
        config_path: _,
        target_url: _,
    } = match preflight(&args).await {
        Ok(ctx) => ctx,
        Err(exit) => return exit,
    };

    // Emoji helpers (resolved once after NO_COLOR check)
    let ok = rust_scraper::cli::preflight::icon("✅", "OK");
    let warn_icon = rust_scraper::cli::preflight::icon("⚠️", "WARN");
    let info_icon = rust_scraper::cli::preflight::icon("📌", "INFO");

    info!(
        "Rust Scraper {} - Clean Architecture",
        rust_scraper::version_string()
    );
    info!("{} Target: {}", info_icon, target_url);
    info!("{} Output: {}", info_icon, args.output.display());

    // Load user agents with TTL-based caching
    info!("Loading user agents (cache check)...");
    let user_agents = UserAgentCache::load().await;
    info!(
        "{} User agent loaded: {} agents available",
        ok,
        user_agents.len()
    );

    // Validate URL
    let parsed_url = match rust_scraper::validate_and_parse_url(&target_url) {
        Ok(url) => url,
        Err(e) => {
            let suggestion = "Use http:// or https:// scheme with a valid host";
            let cli_err = CliError::NetworkError {
                msg: e.to_string(),
                suggestion: suggestion.into(),
            };
            let no_color = rust_scraper::is_no_color();
            eprintln!("{}", format_cli_error(&cli_err, no_color));
            return CliExit::UsageError(e.to_string());
        },
    };

    info!("{} URL validated: {}", ok, parsed_url);

    // Run preflight checks
    let PreflightContext {
        vault_path,
        config_path: _,
        target_url: _,
    } = match rust_scraper::cli::commands::preflight(&args).await {
        Ok(ctx) => ctx,
        Err(exit) => return exit,
    };

    // Create scraper config
    let scraper_config = ScraperConfig {
        download_images: args.download_images,
        download_documents: args.download_documents,
        output_dir: args.output.clone(),
        max_file_size: Some(args.max_file_size),
        download_timeout_secs: args.download_timeout,
        scraper_concurrency: args.concurrency.resolve(),
    };

    if scraper_config.download_images {
        info!("Image download: ENABLED");
    }
    if scraper_config.download_documents {
        info!("Document download: ENABLED");
    }

    // Create crawler config using builder pattern
    let user_agent = get_random_user_agent_from_pool(&user_agents);
    let crawler_config = CrawlerConfig::builder(parsed_url.clone())
        .max_depth(args.max_depth)
        .max_pages(args.max_pages)
        .concurrency(args.concurrency.resolve())
        .delay_ms(args.delay_ms)
        .user_agent(user_agent)
        .timeout_secs(args.timeout_secs)
        .include_patterns(args.include_patterns.clone())
        .exclude_patterns(args.exclude_patterns.clone())
        .use_sitemap(args.use_sitemap)
        .sitemap_url(args.sitemap_url.clone().unwrap_or_default())
        .build();

    // URL Discovery with progress bar
    info!("Discovering URLs...");
    let discovered_urls = discover_urls(&crawler_config, &args).await;
    let discovered_count = discovered_urls.len();

    info!("{} Found {} URLs", ok, discovered_count);

    if discovered_urls.is_empty() {
        warn!("{} No URLs discovered, nothing to scrape", warn_icon);
        return CliExit::Success;
    }

    // Dry-run mode
    if args.dry_run {
        info!("Dry-run mode: printing discovered URLs, no scraping");
        for url in &discovered_urls {
            println!("{}", url);
        }
        return CliExit::Success;
    }

    // Interactive selection or headless mode
    let urls_to_scrape = match select_urls(&discovered_urls, &args, &vault_path).await {
        SelectedUrls::Urls(urls) => urls,
        SelectedUrls::None => return CliExit::Success,
        SelectedUrls::Error(exit) => return exit,
    };

    // Resume mode: filter already-processed URLs
    let (urls_to_scrape, state_store) = apply_resume_mode(urls_to_scrape, &args, &target_url).await;

    if urls_to_scrape.is_empty() {
        info!("All URLs already processed, nothing to do");
        return CliExit::Success;
    }

    // Scraping with progress events via mpsc channel
    let start_time = Instant::now();

    // Create progress channel only if not quiet (--quiet flag skips channel sends)
    // and run progress view concurrently
    let progress_handle = if args.quiet {
        None
    } else {
        let (tx, rx) =
            tokio::sync::mpsc::channel::<rust_scraper::adapters::tui::ScrapeProgress>(100);

        // Spawn progress view task
        let urls_for_progress = urls_to_scrape.clone();
        let handle = tokio::spawn(async move {
            use rust_scraper::adapters::tui::run_progress_view;
            run_progress_view(rx, &urls_for_progress).await
        });

        Some((tx, handle))
    };

    // Extract tx for scraping
    let progress_tx = progress_handle.as_ref().map(|(tx, _)| tx);

    let (results, failures) = scrape_urls(
        &urls_to_scrape,
        &scraper_config,
        &args,
        progress_tx.cloned(),
    )
    .await;

    // Wait for progress view to complete
    if let Some((_, handle)) = progress_handle {
        let _ = handle.await;
    }

    let duration = start_time.elapsed();

    if results.is_empty() && !failures.is_empty() {
        warn!("No content extracted from any URL");
        let suggestion = "Review errors above. Check if the site is blocking scrapers";
        let cli_err = CliError::NetworkError {
            msg: "all URLs failed".into(),
            suggestion: suggestion.into(),
        };
        let no_color = rust_scraper::is_no_color();
        eprintln!("{}", format_cli_error(&cli_err, no_color));
        return CliExit::NetworkError("all URLs failed".into());
    }

    if results.is_empty() {
        warn!("No content extracted, nothing to export");
        return CliExit::Success;
    }

    info!(
        "{} Scraping completed: {} elements extracted",
        ok,
        results.len()
    );

    // Export results
    let processed_urls =
        match run_export_flow(&results, &args, &vault_path, state_store.as_ref()).await {
            Ok(urls) => urls,
            Err(exit) => return exit,
        };

    // Save individual files
    let output_dir = determine_output_dir(&args, &vault_path);
    let obsidian_options = build_obsidian_options(&args, &vault_path);
    export_flow::save_files(&results, &output_dir, &args.format, &obsidian_options);

    // Summary of downloaded assets
    let total_assets: usize = results.iter().map(|r| r.assets.len()).sum();
    if total_assets > 0 {
        info!(
            "Total assets downloaded: {} (images and documents)",
            total_assets
        );
    }

    // Print summary
    let summary = ScrapeSummary {
        urls_discovered: discovered_count,
        urls_scraped: results.len(),
        urls_failed: failures.len(),
        urls_skipped: 0,
        elements_extracted: results.len(),
        assets_downloaded: total_assets,
        duration,
    };

    let no_color = rust_scraper::is_no_color();
    if !args.quiet {
        eprintln!("{}", summary.display(no_color));
    }

    info!("Pipeline completed successfully!");
    info!("Files generated: {}", args.output.display());
    info!("Total URLs processed: {}", urls_to_scrape.len());
    if args.resume {
        info!("Resume mode: processed {} new URLs", processed_urls.len());
    }

    // Return appropriate exit code
    if failures.is_empty() {
        CliExit::Success
    } else {
        CliExit::PartialSuccess {
            success: results.len(),
            failed: failures.len(),
        }
    }
}

// ============================================================================
// Completions Handler (extracted from main)
// ============================================================================

/// Handle the completions subcommand.
pub fn handle_completions(shell: rust_scraper::Shell) -> CliExit {
    use clap_complete::Shell as ClapShell;
    use rust_scraper::cli::completions::generate_completions;
    use rust_scraper::Args;

    let shell: ClapShell = shell.into();
    if let Err(e) = generate_completions::<Args>(shell) {
        eprintln!("Error generating completions: {}", e);
        return CliExit::IoError(e.to_string());
    }
    CliExit::Success
}
