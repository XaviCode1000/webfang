//! Export flow — handles result export (standard and AI-cleaned) and file saving.

use std::path::PathBuf;
#[allow(unused_imports)]
use tracing::{info, warn};

use rust_scraper::cli::error::CliExit;
use rust_scraper::domain::ScrapedContent;
use rust_scraper::infrastructure::export::state_store::StateStore;
use rust_scraper::infrastructure::output::file_saver::ObsidianOptions;
use rust_scraper::{export_factory, save_results, ExportFormat, OutputFormat};

// ============================================================================
// Export Results (RAG pipeline)
// ============================================================================

/// Configuration for the export flow.
#[allow(dead_code)]
pub struct ExportConfig<'a> {
    pub results: &'a [ScrapedContent],
    pub output_dir: PathBuf,
    pub format: OutputFormat,
    pub export_format: ExportFormat,
    pub clean_ai: bool,
    pub quick_save: bool,
    pub vault_path: Option<&'a PathBuf>,
    pub obsidian_options: ObsidianOptions,
    pub state_store: Option<&'a StateStore>,
    pub resume: bool,
    /// AI settings (only used when clean_ai is true and feature is enabled)
    pub ai_threshold: f32,
    pub ai_max_tokens: usize,
    pub ai_offline: bool,
}

/// Run the export flow: AI-cleaned or standard export.
///
/// Returns the list of processed URLs on success.
#[cfg(feature = "ai")]
pub async fn run_export(config: ExportConfig<'_>) -> Result<Vec<String>, CliExit> {
    if config.clean_ai {
        run_ai_export(&config).await
    } else {
        run_standard_export(&config)
    }
}

/// Run the export flow (non-AI build).
#[cfg(not(feature = "ai"))]
pub async fn run_export(config: ExportConfig<'_>) -> Result<Vec<String>, CliExit> {
    if config.clean_ai {
        warn!("--clean-ai requires the 'ai' feature. Recompile with --features ai");
        return Err(CliExit::UsageError(
            "AI semantic cleaning requires --features ai. Recompile with: cargo run --features ai".into(),
        ));
    }
    run_standard_export(&config)
}

/// Standard export path (backward compatible).
fn run_standard_export(config: &ExportConfig<'_>) -> Result<Vec<String>, CliExit> {
    match export_factory::process_results(
        config.results,
        config.output_dir.clone(),
        config.export_format,
        "export",
        config.state_store,
        config.resume,
    ) {
        Ok(urls) => Ok(urls),
        Err(e) => {
            warn!("Failed to export results: {}", e);
            Err(CliExit::IoError(e.to_string()))
        },
    }
}

/// AI semantic cleaning export path.
#[cfg(feature = "ai")]
async fn run_ai_export(config: &ExportConfig<'_>) -> Result<Vec<String>, CliExit> {
    use rust_scraper::domain::DocumentChunk;
    use rust_scraper::infrastructure::ai::semantic_cleaner_impl::{
        ModelConfig, SemanticCleanerImpl,
    };
    use rust_scraper::SemanticCleaner;
    use std::sync::Arc;

    info!("Initializing AI semantic cleaner...");
    let ai_config = ModelConfig::default()
        .with_relevance_threshold(config.ai_threshold)
        .with_max_tokens(config.ai_max_tokens)
        .with_offline_mode(config.ai_offline);
    let cleaner = match SemanticCleanerImpl::new(ai_config).await {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to initialize semantic cleaner: {}", e);
            return Err(CliExit::IoError(format!(
                "Failed to initialize AI semantic cleaner: {}. Ensure ONNX model is available.",
                e
            )));
        },
    };

    info!(
        "Starting AI cleaning for {} pages concurrently...",
        config.results.len()
    );

    let cleaner = Arc::new(cleaner);

    let cleaning_tasks: Vec<_> = config
        .results
        .iter()
        .map(|result| {
            let html_content = result
                .html
                .clone()
                .unwrap_or_else(|| result.content.clone());
            let url = result.url.clone();
            let cleaner = Arc::clone(&cleaner);
            async move {
                let chunks_result = cleaner.clean(&html_content).await;
                (url, chunks_result, result.clone())
            }
        })
        .collect();

    let cleaning_results = futures::future::join_all(cleaning_tasks).await;

    let mut cleaned_chunks: Vec<rust_scraper::domain::DocumentChunk> =
        Vec::with_capacity(config.results.len() * 2);
    for (url, chunks_result, result) in cleaning_results {
        match chunks_result {
            Ok(chunks) => {
                if chunks.is_empty() {
                    warn!("AI cleaner produced 0 chunks for: {}", url);
                    cleaned_chunks.push(DocumentChunk::from_scraped_content(&result));
                } else {
                    cleaned_chunks.extend(chunks);
                }
            },
            Err(e) => {
                warn!(
                    "Failed to clean content for {}: {}. Using fallback.",
                    url, e
                );
                cleaned_chunks.push(DocumentChunk::from_scraped_content(&result));
            },
        }
    }

    info!(
        "AI cleaning complete: {} chunks from {} pages",
        cleaned_chunks.len(),
        config.results.len()
    );

    match export_factory::process_results_with_chunks(
        &cleaned_chunks,
        config.output_dir.clone(),
        config.export_format,
        "export",
        config.state_store,
        config.resume,
    ) {
        Ok(urls) => Ok(urls),
        Err(e) => {
            warn!("Failed to export cleaned results: {}", e);
            Err(CliExit::IoError(e.to_string()))
        },
    }
}

// ============================================================================
// Save Individual Files (Markdown/Text/JSON)
// ============================================================================

/// Save individual output files with Obsidian support.
///
/// This is non-fatal — a failure here doesn't abort the pipeline since
/// RAG export (JSONL) already succeeded.
pub fn save_files(
    results: &[ScrapedContent],
    output_dir: &PathBuf,
    format: &OutputFormat,
    obsidian_options: &ObsidianOptions,
) {
    if let Err(e) = save_results(results, output_dir, format, obsidian_options) {
        warn!("Failed to save individual files: {}", e);
        // Continue — file save is non-fatal, RAG export succeeded
    }
}
