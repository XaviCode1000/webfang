//! Rust Scraper - Modern web scraper for RAG datasets
//!
//! Extracts clean, structured content from web pages using readability algorithm.

use anyhow::Context;
use rust_scraper::{config, scraper, validate_and_parse_url, Args, Parser};
use tracing::{info, warn};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Parsear argumentos CLI - Si no hay URL, error inmediato y claro
    let args = Args::parse();

    // 2. Inicializar logging con nivel configurable
    let log_level = match args.verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    config::init_logging(log_level);

    info!("🚀 Rust Scraper v0.2.0 - Modern Stack");
    info!("📌 Target: {}", args.url);
    info!("📁 Output: {}", args.output.display());

    // 3. Validar URL - parsing con la crate url
    let parsed_url = validate_and_parse_url(&args.url).context("Invalid URL provided")?;

    info!("✅ URL validada: {}", parsed_url);

    // 4. Crear cliente HTTP configurado
    let client = scraper::create_http_client()?;

    // 5. Ejecutar scraping con el nuevo enfoque
    info!("📡 Iniciando scraping...");

    let results = scraper::scrape_with_readability(
        &client,
        &parsed_url,
        &args.selector,
        args.max_pages,
        args.delay_ms,
    )
    .await
    .context("Scraping failed")?;

    if results.is_empty() {
        warn!("⚠️  No se obtuvo contenido de la página");
        return Ok(());
    }

    info!(
        "✅ Scraping completado: {} elementos extraídos",
        results.len()
    );

    // 6. Guardar resultados
    info!("💾 Guardando resultados...");
    scraper::save_results(&results, &args.output, &args.format)?;

    info!("🎉 Pipeline completado exitosamente!");
    info!("📊 Archivos generados: {}", args.output.display());

    Ok(())
}
