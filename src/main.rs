//! Rust Scraper - Modern web scraper for RAG datasets
//!
//! Extracts clean, structured content from web pages using readability algorithm.
//!
//! # Architecture
//!
//! Following Clean Architecture with TUI support:
//!
//! ```text
//! main.rs (Orchestrator) -> CliExit
//!     │
//!     ├─→ Args::try_parse()           ← CLI parsing
//!     ├─→ init_logging_dual()         ← stderr-only tracing
//!     ├─→ ConfigDefaults::load()      ← TOML config
//!     ├─→ pre-flight HEAD check       ← fail fast on DNS errors
//!     │
//!     ├─→ discover_urls_for_tui()     ← Application layer (with spinner)
//!     │       ↓
//!     │    [Vec<Url>]
//!     │       ↓
//!     ├─→ (dry-run: print URLs, exit 0)
//!     │
//!     ├─→ adapters::tui::run_selector() ← Adapter layer (UI, optional)
//!     │       ↓
//!     │    [Vec<Url>] (selected)
//!     │       ↓
//!     └─→ scrape_single_url_for_tui() ← per-URL with progress bar
//! ```
//!
//! **Golden Rule:** Application layer NEVER imports ratatui/crossterm/indicatif.

use clap::Parser;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rust_scraper::infrastructure::obsidian::{detect_vault, open_note};
use rust_scraper::{
    adapters::tui,
    application::{
        discover_urls_for_tui,
        http_client::{HttpClient, HttpClientConfig},
        scrape_single_url_for_tui,
    },
    cli::{
        completions::generate_completions,
        config::ConfigDefaults,
        error::{format_cli_error, CliError, CliExit},
        summary::ScrapeSummary,
    },
    export_factory, save_results, validate_and_parse_url, Args, Commands, CrawlerConfig,
    ObsidianOptions, ScraperConfig, UserAgentCache,
};
use slug::slugify;
use std::path::PathBuf;
use std::time::Instant;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> CliExit {
    // =========================================================================
    // 1. Parse CLI arguments
    // =========================================================================
    let args = match Args::try_parse() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{}", e);
            return CliExit::UsageError("invalid arguments".into());
        },
    };

    // =========================================================================
    // 2. Handle subcommands (completions)
    // =========================================================================
    if let Some(Commands::Completions { shell }) = args.subcommand {
        let shell: clap_complete::Shell = shell.into();
        if let Err(e) = generate_completions::<Args>(shell) {
            eprintln!("Error generating completions: {}", e);
            return CliExit::IoError(e.to_string());
        }
        return CliExit::Success;
    }

    // =========================================================================
    // 2b. URL is required for scraping (subcommands already handled above)
    // =========================================================================
    let target_url = match args.url {
        Some(ref u) => u.clone(),
        None => {
            eprintln!("Error: --url is required for scraping");
            return CliExit::UsageError("--url is required".into());
        },
    };

    // =========================================================================
    // 3. Initialize logging (stderr-only, respects quiet + NO_COLOR)
    // =========================================================================
    let no_color = rust_scraper::is_no_color();
    let log_level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    rust_scraper::init_logging_dual(log_level, args.quiet, no_color);

    // =========================================================================
    // 4. Load config file (graceful: missing file = defaults)
    // =========================================================================
    let config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rust-scraper")
        .join("config.toml");
    let config_defaults = ConfigDefaults::load(&config_path);

    if config_path.exists() {
        info!("Config loaded: {}", config_path.display());
    }

    // =========================================================================
    // 5. Apply config file defaults where CLI args are at default values
    // =========================================================================
    let args = apply_config_defaults(args, &config_defaults);

    // =========================================================================
    // 5b. Vault detection for Obsidian integration
    // =========================================================================
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

    // Emoji helpers (resolved once after NO_COLOR check)
    let ok = icon("✅", "OK");
    let warn_icon = icon("⚠️", "WARN");
    let info_icon = icon("📌", "INFO");

    info!(
        "Rust Scraper {} - Clean Architecture",
        rust_scraper::version_string()
    );
    info!("{} Target: {}", info_icon, target_url);
    info!("{} Output: {}", info_icon, args.output.display());

    // =========================================================================
    // 6. Load user agents with TTL-based caching
    // =========================================================================
    info!("Loading user agents (cache check)...");
    let user_agents = UserAgentCache::load().await;
    info!(
        "{} User agent loaded: {} agents available",
        ok,
        user_agents.len()
    );

    // =========================================================================
    // 7. Validate URL
    // =========================================================================
    let parsed_url = match validate_and_parse_url(&target_url) {
        Ok(url) => url,
        Err(e) => {
            let suggestion = "Use http:// or https:// scheme with a valid host";
            let cli_err = CliError::NetworkError {
                msg: e.to_string(),
                suggestion: suggestion.into(),
            };
            eprintln!("{}", format_cli_error(&cli_err, no_color));
            return CliExit::UsageError(e.to_string());
        },
    };

    info!("{} URL validated: {}", ok, parsed_url);

    // =========================================================================
    // 8. Pre-flight HEAD check (T-070)
    // =========================================================================
    info!("Checking connectivity...");
    match preflight_check(&parsed_url).await {
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
            eprintln!("{}", format_cli_error(&cli_err, no_color));
            return CliExit::NetworkError(msg);
        },
    }

    // =========================================================================
    // 9. Create scraper config
    // =========================================================================
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

    // =========================================================================
    // 10. Create crawler config using builder pattern
    // =========================================================================
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

    // =========================================================================
    // 11. URL Discovery with progress bar (T-080)
    // =========================================================================
    info!("Discovering URLs...");

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

    let discovered_urls = match discover_urls_for_tui(&target_url, &crawler_config).await {
        Ok(urls) => urls,
        Err(e) => {
            if let Some(pb) = discovery_pb.as_ref() {
                pb.finish_with_message("Discovery failed");
            }
            warn!("URL discovery failed: {}", e);
            Vec::new()
        },
    };

    let discovered_count = discovered_urls.len();

    if let Some(pb) = discovery_pb {
        pb.finish_with_message(format!("Found {} URLs", discovered_count).to_owned());
    }

    info!("{} Found {} URLs", ok, discovered_count);

    if discovered_urls.is_empty() {
        warn!("{} No URLs discovered, nothing to scrape", warn_icon);
        return CliExit::Success;
    }

    // =========================================================================
    // 12. Dry-run mode (T-091)
    // =========================================================================
    if args.dry_run {
        info!("Dry-run mode: printing discovered URLs, no scraping");
        for url in &discovered_urls {
            println!("{}", url);
        }
        return CliExit::Success;
    }

    // =========================================================================
    // 13. Interactive selection or headless mode
    // =========================================================================
    let urls_to_scrape = if args.quick_save && vault_path.is_some() {
        // Quick-save mode: bypass TUI, use all discovered URLs
        info!("Quick-save mode: bypassing TUI, will save to vault _inbox");
        discovered_urls
    } else if args.interactive {
        info!("Starting interactive TUI selector...");
        match tui::run_selector(&discovered_urls).await {
            Ok(selected) => {
                info!("{} User selected {} URLs", ok, selected.len());
                if selected.is_empty() {
                    info!("No URLs selected, exiting");
                    return CliExit::Success;
                }
                selected
            },
            Err(tui::TuiError::Interrupted) => {
                info!("User interrupted TUI selector, exiting");
                return CliExit::Success;
            },
            Err(e) => {
                warn!("TUI error: {}", e);
                return CliExit::ProtocolError(e.to_string());
            },
        }
    } else {
        info!(
            "Headless mode: will scrape all {} URLs",
            discovered_urls.len()
        );
        discovered_urls
    };

    // =========================================================================
    // 14. Resume mode: filter already-processed URLs
    // =========================================================================
    let state_store = if args.resume {
        info!("Resume mode enabled - tracking processed URLs");
        let state_dir = args.state_dir.unwrap_or_else(|| {
            let cache_base = std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".cache")
                });
            cache_base.join("rust-scraper").join("state")
        });

        let domain = export_factory::domain_from_url(&target_url);
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

    let urls_to_scrape = if args.resume {
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

    if urls_to_scrape.is_empty() {
        info!("All URLs already processed, nothing to do");
        return CliExit::Success;
    }

    // =========================================================================
    // 15. Scraping with per-URL progress bar (T-081)
    // =========================================================================
    let start_time = Instant::now();

    // Create HTTP client ONCE for all scraping (not per-URL!)
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
            return CliExit::NetworkError(e.to_string());
        },
    };

    let total_urls = urls_to_scrape.len();
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

    for url in &urls_to_scrape {
        if let Some(pb) = scrape_pb.as_ref() {
            pb.set_message(format!("Scraping: {}", url.host_str().unwrap_or("unknown")));
        }

        match scrape_single_url_for_tui(http_client.client(), url, &scraper_config).await {
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

    let duration = start_time.elapsed();

    if results.is_empty() && !failures.is_empty() {
        warn!("No content extracted from any URL");
        let suggestion = "Review errors above. Check if the site is blocking scrapers";
        let cli_err = CliError::NetworkError {
            msg: "all URLs failed".into(),
            suggestion: suggestion.into(),
        };
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

    // =========================================================================
    // 16. Export results
    // =========================================================================
    info!("Exporting results (format: {:?})...", args.export_format);

    let processed_urls = if args.clean_ai {
        // AI semantic cleaning path
        #[cfg(feature = "ai")]
        {
            use rust_scraper::infrastructure::ai::semantic_cleaner_impl::{ModelConfig, SemanticCleanerImpl};
            use rust_scraper::domain::DocumentChunk;
            use rust_scraper::SemanticCleaner;
            
            info!("Initializing AI semantic cleaner...");
            let config = ModelConfig::default()
                .with_relevance_threshold(args.threshold)
                .with_max_tokens(args.max_tokens)
                .with_offline_mode(args.offline);
            let cleaner = match SemanticCleanerImpl::new(config).await {
                Ok(c) => c,
                Err(e) => {
                    warn!("Failed to initialize semantic cleaner: {}", e);
                    return CliExit::IoError(format!(
                        "Failed to initialize AI semantic cleaner: {}. Ensure ONNX model is available.",
                        e
                    ));
                }
            };

            // Pre-clean all content
            let mut cleaned_chunks: Vec<rust_scraper::domain::DocumentChunk> = Vec::with_capacity(results.len() * 2);
            for result in &results {
                // Use the `html` field from ScrapedContent - it's Option<String>
                let html_content = result.html.as_deref().unwrap_or(&result.content);
                let chunks_result: Result<Vec<rust_scraper::domain::DocumentChunk>, _> = cleaner.clean(html_content).await;
                match chunks_result {
                    Ok(chunks) => {
                        if chunks.is_empty() {
                            warn!("AI cleaner produced 0 chunks for: {}", result.url);
                            // Fallback: use non-AI conversion for this page
                            cleaned_chunks.push(DocumentChunk::from_scraped_content(result));
                        } else {
                            cleaned_chunks.extend(chunks);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to clean content for {}: {}. Using fallback.", result.url, e);
                        cleaned_chunks.push(DocumentChunk::from_scraped_content(result));
                    }
                }
            }

            info!("AI cleaning complete: {} chunks from {} pages", cleaned_chunks.len(), results.len());

            // Export cleaned chunks
            match export_factory::process_results_with_chunks(
                &cleaned_chunks,
                args.output.clone(),
                args.export_format,
                "export",
                state_store.as_ref(),
                args.resume,
            ) {
                Ok(urls) => urls,
                Err(e) => {
                    warn!("Failed to export cleaned results: {}", e);
                    return CliExit::IoError(e.to_string());
                },
            }
        }

        #[cfg(not(feature = "ai"))]
        {
            warn!("--clean-ai requires the 'ai' feature. Recompile with --features ai");
            return CliExit::UsageError("AI semantic cleaning requires --features ai. Recompile with: cargo run --features ai".into());
        }
    } else {
        // Standard export path (backward compatible)
        match export_factory::process_results(
            &results,
            args.output.clone(),
            args.export_format,
            "export",
            state_store.as_ref(),
            args.resume,
        ) {
            Ok(urls) => urls,
            Err(e) => {
                warn!("Failed to export results: {}", e);
                return CliExit::IoError(e.to_string());
            },
        }
    };

    // =========================================================================
    // 16b. Save individual files (Markdown/Text/JSON with Obsidian support)
    // =========================================================================

    // Determine output directory (vault _inbox for quick-save mode)
    let output_dir = if args.quick_save {
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
    };

    let obsidian_options = ObsidianOptions {
        wiki_links: args.obsidian_wiki_links,
        tags: args.obsidian_tags.clone().unwrap_or_default(),
        relative_assets: args.obsidian_relative_assets,
        rich_metadata: args.obsidian_rich_metadata,
        quick_save: args.quick_save,
        vault_path: vault_path.clone(),
    };

    if let Err(e) = save_results(&results, &output_dir, &args.format, &obsidian_options) {
        warn!("Failed to save individual files: {}", e);
        // Continue - file save is non-fatal, RAG export succeeded
    }

    // =========================================================================
    // 16c. Open in Obsidian (if vault detected and requested)
    // =========================================================================
    if vault_path.is_some() && args.obsidian_rich_metadata {
        // Try to open the saved notes in Obsidian
        for item in &results {
            let file_path = if args.quick_save {
                // Calculate the filename from the URL
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                let url_str = item.url.as_str();
                // Use url.path() which returns a PathBuf (owned)
                let url = url::Url::parse(url_str).ok();
                let path_segment = url
                    .as_ref()
                    .and_then(|u| u.path_segments())
                    .and_then(|mut p| p.next_back())
                    .unwrap_or("untitled");
                let slug = slugify(path_segment);
                format!("_inbox/{}-{}.md", today, slug)
            } else {
                // Use domain-based folder structure
                let url_str = item.url.as_str();
                let domain = export_factory::domain_from_url(url_str);
                let url = url::Url::parse(url_str).ok();
                let path_segment = url
                    .as_ref()
                    .and_then(|u| u.path_segments())
                    .and_then(|mut p| p.next_back())
                    .unwrap_or("index");
                let slug = slugify(path_segment);
                format!("{}/{}.md", domain, slug)
            };

            if let Some(ref vault) = vault_path {
                match open_note(vault, std::path::Path::new(&file_path)) {
                    Ok(()) => info!("Opened in Obsidian: {}", item.title),
                    Err(e) => warn!("Failed to open in Obsidian: {}", e),
                }
            }
        }
    }

    // Summary of downloaded assets
    let total_assets: usize = results.iter().map(|r| r.assets.len()).sum();
    if total_assets > 0 {
        info!(
            "Total assets downloaded: {} (images and documents)",
            total_assets
        );
    }

    // =========================================================================
    // 17. Print summary (T-092)
    // =========================================================================
    let summary = ScrapeSummary {
        urls_discovered: discovered_count,
        urls_scraped: results.len(),
        urls_failed: failures.len(),
        urls_skipped: 0,
        elements_extracted: results.len(),
        assets_downloaded: total_assets,
        duration,
    };

    if !args.quiet {
        eprintln!("{}", summary.display(no_color));
    }

    info!("Pipeline completed successfully!");
    info!("Files generated: {}", args.output.display());
    info!("Total URLs processed: {}", urls_to_scrape.len());
    if args.resume {
        info!("Resume mode: processed {} new URLs", processed_urls.len());
    }

    // =========================================================================
    // 18. Return appropriate exit code
    // =========================================================================
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
// Pre-flight Check (T-070)
// ============================================================================

enum PreflightResult {
    /// 2xx or 3xx response — all good
    Ok,
    /// 4xx or 5xx response — connectivity OK but server issue
    Warning(u16),
    /// DNS failure, connection refused, timeout — cannot reach host
    Failed(String),
}

/// Send a HEAD request to verify connectivity before starting discovery.
///
/// Returns `PreflightResult::Ok` for 2xx/3xx, `Warning` for 4xx/5xx,
/// and `Failed` for DNS/connection errors.
async fn preflight_check(url: &url::Url) -> PreflightResult {
    let client = match rust_scraper::create_http_client() {
        Ok(c) => c,
        Err(e) => return PreflightResult::Failed(format!("failed to create HTTP client: {}", e)),
    };

    match client.head(url.as_str()).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            if status < 400 {
                PreflightResult::Ok
            } else {
                PreflightResult::Warning(status)
            }
        },
        Err(e) => {
            if e.is_timeout() {
                PreflightResult::Failed("connection timed out".into())
            } else if e.is_connect() {
                PreflightResult::Failed(format!("connection refused: {}", e))
            } else {
                PreflightResult::Failed(format!("network error: {}", e))
            }
        },
    }
}

// ============================================================================
// Config Merge Helper
// ============================================================================

/// Apply config file defaults where CLI args are still at their hardcoded defaults.
///
/// Precedence: CLI > env (handled by clap) > config file > struct defaults.
fn apply_config_defaults(mut args: Args, config: &ConfigDefaults) -> Args {
    use rust_scraper::{ConcurrencyConfig, ExportFormat, OutputFormat};

    if let Some(ref fmt) = config.format {
        let target = match fmt.to_lowercase().as_str() {
            "markdown" => OutputFormat::Markdown,
            "json" => OutputFormat::Json,
            "text" => OutputFormat::Text,
            _ => OutputFormat::Markdown,
        };
        if args.format == OutputFormat::Markdown && target != OutputFormat::Markdown {
            args.format = target;
        }
    }

    if let Some(ref fmt) = config.export_format {
        let target = match fmt.to_lowercase().as_str() {
            "jsonl" => ExportFormat::Jsonl,
            "vector" => ExportFormat::Vector,
            "auto" => ExportFormat::Auto,
            _ => ExportFormat::Jsonl,
        };
        if args.export_format == ExportFormat::Jsonl && target != ExportFormat::Jsonl {
            args.export_format = target;
        }
    }

    if let Some(ref c) = config.concurrency {
        // ConcurrencyConfig doesn't implement PartialEq, so check via is_auto()
        if args.concurrency.is_auto() {
            args.concurrency = ConcurrencyConfig::from(c.as_str());
        }
    }

    if let Some(ref s) = config.selector {
        if args.selector == "body" {
            args.selector = s.clone();
        }
    }

    if let Some(n) = config.max_pages {
        if args.max_pages == 10 {
            args.max_pages = n;
        }
    }

    if let Some(n) = config.delay_ms {
        if args.delay_ms == 1000 {
            args.delay_ms = n;
        }
    }

    if let Some(v) = config.use_sitemap {
        if !args.use_sitemap && v {
            args.use_sitemap = v;
        }
    }

    // Obsidian config
    if let Some(ref tags_str) = config.obsidian_tags {
        if args.obsidian_tags.is_none() {
            args.obsidian_tags = Some(
                tags_str
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect(),
            );
        }
    }
    if let Some(v) = config.obsidian_wiki_links {
        if !args.obsidian_wiki_links && v {
            args.obsidian_wiki_links = v;
        }
    }
    if let Some(v) = config.obsidian_relative_assets {
        if !args.obsidian_relative_assets && v {
            args.obsidian_relative_assets = v;
        }
    }
    if let Some(ref vault) = config.vault_path {
        if args.vault.is_none() {
            args.vault = Some(PathBuf::from(vault));
        }
    }

    args
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Return emoji or ASCII equivalent based on NO_COLOR setting.
#[inline]
fn icon(emoji: &str, ascii: &str) -> String {
    if rust_scraper::should_emit_emoji() {
        emoji.to_string()
    } else {
        ascii.to_string()
    }
}

/// Get random user agent from pool.
fn get_random_user_agent_from_pool(user_agents: &[String]) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let index = rng.gen_range(0..user_agents.len());
    user_agents[index].clone()
}
