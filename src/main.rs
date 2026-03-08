//! Rust Scraper - Modern web scraper for RAG datasets
//!
//! Extracts clean, structured content from web pages using readability algorithm.

use anyhow::Context;
use rust_scraper::{
    create_http_client, save_results, scrape_with_config, validate_and_parse_url, Args, Parser,
    ScraperConfig,
};
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

    info!("🚀 Rust Scraper v0.3.0 - Clean Architecture");
    info!("📌 Target: {}", args.url);
    info!("📁 Output: {}", args.output.display());

    // 3. Validate URL - parse with url crate
    let parsed_url = validate_and_parse_url(&args.url).context("Invalid URL provided")?;

    info!("✅ URL validated: {}", parsed_url);

    // 4. Create configured HTTP client (with retry + user-agent rotation)
    let client = create_http_client()?;

    // 5. Configure scraping with download options
    let config = ScraperConfig {
        download_images: args.download_images,
        download_documents: args.download_documents,
        output_dir: args.output.clone(),
        max_file_size: Some(50 * 1024 * 1024), // 50MB default
    };

    if config.download_images {
        info!("🖼️  Image download: ENABLED");
    }
    if config.download_documents {
        info!("📄 Document download: ENABLED");
    }

    // 6. Execute scraping
    info!("📡 Starting scraping...");

    let results = scrape_with_config(&client, &parsed_url, &config)
        .await
        .context("Scraping failed")?;

    if results.is_empty() {
        warn!("⚠️  No content extracted from page");
        return Ok(());
    }

    info!(
        "✅ Scraping completed: {} elements extracted",
        results.len()
    );

    // 7. Save results
    info!("💾 Saving results...");
    save_results(&results, &args.output, &args.format)?;

    // Summary of downloaded assets
    let total_assets: usize = results.iter().map(|r| r.assets.len()).sum();
    if total_assets > 0 {
        info!(
            "📦 Total assets downloaded: {} (images and documents)",
            total_assets
        );
    }

    info!("🎉 Pipeline completed successfully!");
    info!("📊 Files generated: {}", args.output.display());

    Ok(())
}
