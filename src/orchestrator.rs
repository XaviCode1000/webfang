//! Main orchestrator — coordinates the full scraping pipeline.
//!
//! The orchestrator handles URL discovery, TUI selection, scraping, and export.
//! It delegates config merging to `preflight` and export logic to `export_flow`.

use std::path::PathBuf;
use std::time::Instant;
use tracing::{info, warn};

use rust_scraper::application::{
    discover_urls_for_tui,
    http_client::{HttpClient, HttpClientConfig},
    scrape_single_url_for_tui,
};
use rust_scraper::cli::error::{format_cli_error, CliError, CliExit};
use rust_scraper::cli::summary::ScrapeSummary;
use rust_scraper::infrastructure::obsidian::detect_vault;
use rust_scraper::{
    adapters, get_random_user_agent_from_pool, export_factory, Args, CrawlerConfig,
    ObsidianOptions, ScraperConfig, UserAgentCache,
};

use crate::export_flow::{self, ExportConfig};
use crate::preflight::{self, PreflightResult};

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

    // Emoji helpers (resolved once after NO_COLOR check)
    let ok = preflight::icon("✅", "OK");
    let warn_icon = preflight::icon("⚠️", "WARN");
    let info_icon = preflight::icon("📌", "INFO");

    // Vault detection
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust-scraper")
        .join("config.toml");
    let config_defaults = rust_scraper::cli::config::ConfigDefaults::load(&config_path);

    if config_path.exists() {
        info!("Config loaded: {}", config_path.display());
    }

    let vault_path = detect_vault(
        args.vault.as_deref(),
        None,
        config_defaults.vault_path.as_deref(),
    );

    if let Some(ref vault) = vault_path {
        info!("Obsidian vault detected: {}", vault.display());
    } else {
        info!("No Obsidian vault detected, using output directory");
    }

    // GAP 3 (Bug #30): Warn when vault is provided but headless mode (no --quick-save)
    if let Some(ref _vault) = vault_path {
        if !args.quick_save {
            warn!("Vault path provided but --quick-save not enabled.");
            warn!("   Files will be saved to ./output/, not to the vault.");
            warn!("   Use --quick-save to save directly to vault _inbox.");
        }
    }

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

    // Pre-flight HEAD check
    info!("Checking connectivity...");
    match preflight::preflight_check(&parsed_url).await {
        PreflightResult::Ok => {
            info!("{} Connectivity check passed", ok);
        },
        PreflightResult::Warning(status) => {
            warn!(
                "{} Server returned {} but connectivity OK",
                warn_icon, status
            );
        },
        PreflightResult::Failed(msg) => {
            let suggestion = "Check your network connection and URL. Verify the host is reachable";
            let cli_err = CliError::PreflightFailed {
                msg: msg.clone(),
                suggestion: suggestion.into(),
            };
            let no_color = rust_scraper::is_no_color();
            eprintln!("{}", format_cli_error(&cli_err, no_color));
            return CliExit::NetworkError(msg);
        },
    }

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

    // Scraping with per-URL progress bar
    let start_time = Instant::now();
    let (results, failures) = scrape_urls(&urls_to_scrape, &scraper_config, &args).await;
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
    let processed_urls = match run_export_flow(&results, &args, &vault_path, state_store.as_ref()).await {
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
// Sub-functions (extracted for readability)
// ============================================================================

/// Result of URL selection.
enum SelectedUrls {
    Urls(Vec<url::Url>),
    None,    // User cancelled or no selection
    Error(CliExit),
}

/// Discover URLs with progress bar.
async fn discover_urls(crawler_config: &CrawlerConfig, args: &Args) -> Vec<url::Url> {
    use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

    let target_url = args.url.as_ref().expect("url required");

    let discovery_pb = if !args.quiet {
        let pb = ProgressBar::new_spinner();
        pb.set_draw_target(ProgressDrawTarget::stderr());
        pb.enable_steady_tick(std::time::Duration::from_millis(100));
        pb.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner} {msg}")
                .expect("valid spinner template"),
        );
        pb.set_message("Discovering URLs...");
        Some(pb)
    } else {
        None
    };

    let discovered_urls = match discover_urls_for_tui(target_url, crawler_config).await {
        Ok(urls) => urls,
        Err(e) => {
            if let Some(pb) = discovery_pb.as_ref() {
                pb.finish_with_message("Discovery failed");
            }
            warn!("URL discovery failed: {}", e);
            Vec::new()
        },
    };

    if let Some(pb) = discovery_pb {
        pb.finish_with_message(format!("Found {} URLs", discovered_urls.len()).to_owned());
    }

    discovered_urls
}

/// Select URLs via TUI, quick-save, or headless mode.
async fn select_urls(
    discovered_urls: &[url::Url],
    args: &Args,
    vault_path: &Option<PathBuf>,
) -> SelectedUrls {
    let ok = preflight::icon("✅", "OK");

    if args.quick_save && vault_path.is_some() {
        info!("Quick-save mode: bypassing TUI, will save to vault _inbox");
        SelectedUrls::Urls(discovered_urls.to_vec())
    } else if args.interactive {
        info!("Starting interactive TUI selector...");
        match adapters::tui::run_selector(discovered_urls).await {
            Ok(selected) => {
                info!("{} User selected {} URLs", ok, selected.len());
                if selected.is_empty() {
                    info!("No URLs selected, exiting");
                    SelectedUrls::None
                } else {
                    SelectedUrls::Urls(selected)
                }
            },
            Err(adapters::tui::TuiError::Interrupted) => {
                info!("User interrupted TUI selector, exiting");
                SelectedUrls::None
            },
            Err(e) => {
                warn!("TUI error: {}", e);
                SelectedUrls::Error(CliExit::ProtocolError(e.to_string()))
            },
        }
    } else {
        info!(
            "Headless mode: will scrape all {} URLs",
            discovered_urls.len()
        );
        SelectedUrls::Urls(discovered_urls.to_vec())
    }
}

/// Apply resume mode filtering.
async fn apply_resume_mode(
    urls_to_scrape: Vec<url::Url>,
    args: &Args,
    target_url: &str,
) -> (Vec<url::Url>, Option<rust_scraper::infrastructure::export::state_store::StateStore>) {
    let state_store = if args.resume {
        info!("Resume mode enabled - tracking processed URLs");
        let state_dir = args.state_dir.clone().unwrap_or_else(|| {
            let cache_base = std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".cache")
                });
            cache_base.join("rust-scraper").join("state")
        });

        let domain = export_factory::domain_from_url(target_url);
        info!("State store domain: {}", domain);
        match export_factory::create_state_store(state_dir, &domain) {
            Ok(store) => Some(store),
            Err(e) => {
                warn!("Failed to create state store: {}", e);
                None
            },
        }
    } else {
        None
    };

    let filtered = if args.resume {
        if let Some(store) = state_store.as_ref() {
            match store.load_or_default() {
                Ok(state) => {
                    let original_count = urls_to_scrape.len();
                    let filtered: Vec<_> = urls_to_scrape
                        .into_iter()
                        .filter(|url| {
                            let should_skip = store.is_processed(&state, url.as_str());
                            if should_skip {
                                info!("Skipping already processed: {}", url);
                            }
                            !should_skip
                        })
                        .collect();

                    let skipped_count = original_count - filtered.len();
                    info!(
                        "Resume mode: {} URLs already processed, {} new URLs to scrape",
                        skipped_count,
                        filtered.len()
                    );

                    filtered
                },
                Err(e) => {
                    warn!("Failed to load state: {}", e);
                    urls_to_scrape
                },
            }
        } else {
            urls_to_scrape
        }
    } else {
        urls_to_scrape
    };

    (filtered, state_store)
}

/// Scrape all URLs with progress bar.
async fn scrape_urls(
    urls: &[url::Url],
    scraper_config: &ScraperConfig,
    args: &Args,
) -> (Vec<rust_scraper::domain::ScrapedContent>, Vec<(String, String)>) {
    use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};

    let http_config = HttpClientConfig {
        max_retries: args.max_retries,
        backoff_base_ms: args.backoff_base_ms,
        backoff_max_ms: args.backoff_max_ms,
        accept_language: args.accept_language.clone(),
        ..HttpClientConfig::default()
    };
    let http_client = match HttpClient::new(http_config) {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to create HTTP client: {}", e);
            return (Vec::new(), vec![("http_client".into(), e.to_string())]);
        },
    };

    let total_urls = urls.len();
    let scrape_pb = if !args.quiet {
        let pb = ProgressBar::new(total_urls as u64);
        pb.set_draw_target(ProgressDrawTarget::stderr());
        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{bar:40.cyan/blue}] {pos}/{len} | {msg}")
                .expect("valid progress bar template")
                .progress_chars("=>-"),
        );
        Some(pb)
    } else {
        None
    };

    let mut results = Vec::with_capacity(total_urls);
    let mut failures: Vec<(String, String)> = Vec::new();

    for url in urls {
        if let Some(pb) = scrape_pb.as_ref() {
            pb.set_message(format!("Scraping: {}", url.host_str().unwrap_or("unknown")));
        }

        match scrape_single_url_for_tui(http_client.client(), url, scraper_config).await {
            Ok(content) => {
                results.push(content);
            },
            Err(e) => {
                let url_str = url.as_str().to_string();
                let err_msg = e.to_string();
                warn!("Failed to scrape {}: {}", url_str, err_msg);
                failures.push((url_str, err_msg));
            },
        }

        if let Some(pb) = scrape_pb.as_ref() {
            pb.inc(1);
        }
    }

    if let Some(pb) = scrape_pb {
        let success_count = results.len();
        let fail_count = failures.len();
        pb.finish_with_message(
            format!(
                "Scraping complete: {} succeeded, {} failed",
                success_count, fail_count
            )
            .to_owned(),
        );
    }

    (results, failures)
}

/// Run the export flow (AI or standard).
async fn run_export_flow(
    results: &[rust_scraper::domain::ScrapedContent],
    args: &Args,
    vault_path: &Option<PathBuf>,
    state_store: Option<&rust_scraper::infrastructure::export::state_store::StateStore>,
) -> Result<Vec<String>, CliExit> {
    let ok = preflight::icon("✅", "OK");
    info!("Exporting results (format: {:?})...", args.export_format);

    let output_dir = determine_output_dir(args, vault_path);
    let obsidian_options = build_obsidian_options(args, vault_path);

    let export_config = build_export_config(
        results,
        args,
        output_dir,
        vault_path,
        obsidian_options,
        state_store,
    );

    let processed_urls = match export_flow::run_export(export_config).await {
        Ok(urls) => urls,
        Err(exit) => return Err(exit),
    };

    info!(
        "{} Export completed: {} URLs processed",
        ok,
        processed_urls.len()
    );

    Ok(processed_urls)
}

/// Build ExportConfig from args, handling feature-gated AI fields.
fn build_export_config<'a>(
    results: &'a [rust_scraper::domain::ScrapedContent],
    args: &'a Args,
    output_dir: PathBuf,
    vault_path: &'a Option<PathBuf>,
    obsidian_options: ObsidianOptions,
    state_store: Option<&'a rust_scraper::infrastructure::export::state_store::StateStore>,
) -> ExportConfig<'a> {
    ExportConfig {
        results,
        output_dir,
        format: args.format,
        export_format: args.export_format,
        clean_ai: args.clean_ai,
        quick_save: args.quick_save,
        vault_path: vault_path.as_ref(),
        obsidian_options,
        state_store,
        resume: args.resume,
        #[cfg(feature = "ai")]
        ai_threshold: args.threshold,
        #[cfg(feature = "ai")]
        ai_max_tokens: args.max_tokens,
        #[cfg(feature = "ai")]
        ai_offline: args.offline,
        #[cfg(not(feature = "ai"))]
        ai_threshold: 0.3,
        #[cfg(not(feature = "ai"))]
        ai_max_tokens: 512,
        #[cfg(not(feature = "ai"))]
        ai_offline: false,
    }
}

/// Determine output directory (vault _inbox for quick-save mode).
fn determine_output_dir(args: &Args, vault_path: &Option<PathBuf>) -> PathBuf {
    if args.quick_save {
        if let Some(ref vault) = vault_path {
            let inbox_path = vault.join("_inbox");
            if let Err(e) = std::fs::create_dir_all(&inbox_path) {
                warn!("Failed to create vault _inbox directory: {}", e);
                args.output.clone()
            } else {
                info!("Quick-save: using vault inbox {}", inbox_path.display());
                inbox_path
            }
        } else {
            warn!("Quick-save mode but no vault detected, using output directory");
            args.output.clone()
        }
    } else {
        args.output.clone()
    }
}

/// Build ObsidianOptions from CLI args.
fn build_obsidian_options(args: &Args, vault_path: &Option<PathBuf>) -> ObsidianOptions {
    ObsidianOptions {
        wiki_links: args.obsidian_wiki_links,
        tags: args.obsidian_tags.clone().unwrap_or_default(),
        relative_assets: args.obsidian_relative_assets,
        rich_metadata: args.obsidian_rich_metadata,
        quick_save: args.quick_save,
        vault_path: vault_path.clone(),
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
