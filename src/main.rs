//! Rust Scraper - Modern web scraper for RAG datasets
//!
//! Extracts clean, structured content from web pages using readability algorithm.

mod config;
mod scraper;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use std::path::PathBuf;
use tracing::{info, warn};

/// CLI Arguments - URL es OBLIGATORIA, no hay default
#[derive(Parser, Debug)]
#[command(name = "rust-scraper")]
#[command(about = "Modern web scraper for RAG datasets with clean content extraction", long_about = None)]
struct Args {
    /// URL objetivo a scrapear (OBLIGATORIA)
    /// Ejemplo: https://example.com/article
    #[arg(short, long, required = true, help = "URL to scrape (required)")]
    url: String,

    /// Selector CSS opcional para extraer contenido específico
    /// Si no se especifica, extrae todo el contenido legible
    #[arg(short, long, default_value = "body", help = "CSS selector (optional)")]
    selector: String,

    /// Directorio de salida para los archivos generados
    #[arg(short, long, default_value = "output", help = "Output directory")]
    output: PathBuf,

    /// Formato de salida
    #[arg(short, long, default_value = "markdown", value_enum)]
    format: OutputFormat,

    ///延迟 entre requests (en milisegundos)
    #[arg(long, default_value = "1000", help = "Delay between requests (ms)")]
    delay_ms: u64,

    /// Máximo de páginas a scrapear
    #[arg(long, default_value = "10", help = "Maximum pages to scrape")]
    max_pages: usize,

    /// Verbosity del logging
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    /// Markdown format (recomendado para RAG)
    Markdown,
    /// Plain text sin formato
    Text,
    /// JSON estructurado
    Json,
}

impl Default for Args {
    fn default() -> Self {
        // NO HAY DEFAULT PARA URL - esto fuerza al usuario a especificar
        Self {
            url: String::new(), // Empty - will fail if not provided
            selector: "body".to_string(),
            output: PathBuf::from("output"),
            format: OutputFormat::Markdown,
            delay_ms: 1000,
            max_pages: 10,
            verbose: 0,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
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

/// Valida y parsea una URL - retorna error claro si es inválida
fn validate_and_parse_url(url: &str) -> Result<url::Url> {
    // Basic check first
    if url.is_empty() {
        anyhow::bail!("URL cannot be empty");
    }

    if !url.starts_with("http://") && !url.starts_with("https://") {
        anyhow::bail!("URL must start with http:// or https://");
    }

    // Parse with url crate
    let parsed = url::Url::parse(url).context("Failed to parse URL - check format")?;

    // Validar que tiene host
    if parsed.host_str().is_none() {
        anyhow::bail!("URL must have a valid host");
    }

    Ok(parsed)
}
