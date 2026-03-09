//! Rust Scraper - Modern web scraper for RAG datasets
//!
//! Extracts clean, structured content from web pages using readability algorithm.
//!
//! # Architecture
//!
//! Following Clean Architecture with TUI support (FASE 4):
//!
//! ```text
//! main.rs (Orchestrator)
//!     │
//!     ├─→ discover_urls_for_tui()     ← Application layer (pure)
//!     │       ↓
//!     │    [Vec<Url>]
//!     │       ↓
//!     ├─→ adapters::tui::run_selector() ← Adapter layer (UI)
//!     │       ↓
//!     │    [Vec<Url>] (selected)
//!     │       ↓
//!     └─→ scrape_urls_for_tui()       ← Application layer (pure)
//! ```
//!
//! **Golden Rule:** Application layer NEVER imports ratatui/crossterm.

use anyhow::Context;
use clap::Parser;
use rust_scraper::{
    adapters::tui,
    application::{discover_urls_for_tui, scrape_urls_for_tui},
    export_factory, validate_and_parse_url, Args, CrawlerConfig, ExportFormat, ScraperConfig,
    UserAgentCache,
};
use std::path::PathBuf;
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Parse CLI arguments - Fail fast if URL is missing
    let args = Args::parse();

    // 2. Initialize logging with configurable level
    let log_level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    rust_scraper::config::init_logging(log_level);

    info!("🚀 Rust Scraper v0.4.0 - Clean Architecture + TUI");
    info!("📌 Target: {}", args.url);
    info!("📁 Output: {}", args.output.display());

    // 3. Load user agents with TTL-based caching (TASK-001)
    info!("🔄 Loading user agents (cache check)...");
    let user_agents = UserAgentCache::load().await;
    info!(
        "✅ User agent loaded: {} agents available",
        user_agents.len()
    );

    // 4. Validate URL - parse with url crate (TASK-003: RFC 3986 compliant)
    let parsed_url = validate_and_parse_url(&args.url).context("Invalid URL provided")?;

    info!("✅ URL validated: {}", parsed_url);

    // 5. Create scraper config
    let scraper_config = ScraperConfig {
        download_images: args.download_images,
        download_documents: args.download_documents,
        output_dir: args.output.clone(),
        max_file_size: Some(50 * 1024 * 1024), // 50MB default
        scraper_concurrency: args.concurrency.resolve(), // Auto-detected or explicit
    };

    if scraper_config.download_images {
        info!("🖼️  Image download: ENABLED");
    }
    if scraper_config.download_documents {
        info!("📄 Document download: ENABLED");
    }

    // 6. Create crawler config for URL discovery using builder pattern
    let user_agent = get_random_user_agent_from_pool(&user_agents);
    let crawler_config = CrawlerConfig::builder(parsed_url.clone())
        .max_depth(2)
        .max_pages(args.max_pages)
        .concurrency(args.concurrency.resolve()) // Auto-detected or explicit
        .delay_ms(args.delay_ms)
        .user_agent(user_agent)
        .timeout_secs(30)
        .use_sitemap(args.use_sitemap)
        .sitemap_url(args.sitemap_url.clone().unwrap_or_default())
        .build();

    // 7. FASE 4: TUI Interactive Mode (optional)
    info!("🔍 Discovering URLs...");
    let discovered_urls = discover_urls_for_tui(&args.url, &crawler_config)
        .await
        .context("URL discovery failed")?;

    info!("✅ Found {} URLs", discovered_urls.len());

    if discovered_urls.is_empty() {
        warn!("⚠️  No URLs discovered, nothing to scrape");
        return Ok(());
    }

    // 8. Interactive selection (ONLY if --interactive flag)
    let urls_to_scrape = if args.interactive {
        info!("🎮 Starting interactive TUI selector...");
        match tui::run_selector(&discovered_urls).await {
            Ok(selected) => {
                info!("✅ User selected {} URLs", selected.len());
                if selected.is_empty() {
                    info!("ℹ️  No URLs selected, exiting");
                    return Ok(());
                }
                selected
            }
            Err(tui::TuiError::Interrupted) => {
                info!("ℹ️  User interrupted TUI selector, exiting");
                return Ok(());
            }
            Err(e) => {
                return Err(anyhow::anyhow!("TUI error: {}", e));
            }
        }
    } else {
        // Headless mode: scrape all discovered URLs
        info!(
            "📡 Headless mode: will scrape all {} URLs",
            discovered_urls.len()
        );
        discovered_urls
    };

    // 9. Check Zvec feature availability
    if args.export_format == ExportFormat::Zvec
        && !rust_scraper::infrastructure::export::zvec_exporter::ZvecExporter::is_available()
    {
        warn!(
            "⚠️  Zvec format requested but not available. Enable with: cargo build --features zvec"
        );
        return Err(anyhow::anyhow!(
            "Zvec feature not enabled. Use: cargo build --features zvec"
        ));
    }

    // 9b. Initialize StateStore for resume mode
    let state_store = if args.resume {
        info!("🎯 Resume mode enabled - tracking processed URLs");
        let state_dir = args.state_dir.unwrap_or_else(|| {
            // Build state directory path
            let cache_base = std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join(".cache")
                });
            cache_base.join("rust-scraper").join("state")
        });

        Some(export_factory::create_state_store(state_dir, &args.url)?)
    } else {
        None
    };

    // 9c. Scrape selected URLs
    info!("🕷️  Scraping {} URLs...", urls_to_scrape.len());
    let all_results = scrape_urls_for_tui(&urls_to_scrape, &scraper_config)
        .await
        .context("Scraping failed")?;

    if all_results.is_empty() {
        warn!("⚠️  No content extracted from any URL");
        return Ok(());
    }

    info!(
        "✅ Scraping completed: {} elements extracted",
        all_results.len()
    );

    // 10. Export results
    info!("💾 Exporting results (format: {:?})...", args.export_format);

    let processed_urls = export_factory::process_results(
        &all_results,
        args.output.clone(),
        args.export_format,
        "export",
        state_store.as_ref(),
        args.resume,
    )?;

    // Summary of downloaded assets
    let total_assets: usize = all_results.iter().map(|r| r.assets.len()).sum();
    if total_assets > 0 {
        info!(
            "📦 Total assets downloaded: {} (images and documents)",
            total_assets
        );
    }

    info!("🎉 Pipeline completed successfully!");
    info!("📊 Files generated: {}", args.output.display());
    info!("📈 Total URLs processed: {}", urls_to_scrape.len());
    if args.resume {
        info!(
            "🔄 Resume mode: processed {} new URLs",
            processed_urls.len()
        );
    }

    Ok(())
}

/// Get random user agent from pool
///
/// Helper function to get a random user agent.
fn get_random_user_agent_from_pool(user_agents: &[String]) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let index = rng.gen_range(0..user_agents.len());
    user_agents[index].clone()
}
