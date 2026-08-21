//! Export flow — handles result export (standard and AI-cleaned) and file saving.

use std::path::{Path, PathBuf};
use tracing::warn;

use crate::cli::error::CliExit;
use crate::domain::ScrapedContent;
use crate::infrastructure::export::state_store::StateStore;
use crate::infrastructure::output::file_saver::ObsidianOptions;
use crate::{
    application::export_factory, infrastructure::output::file_saver::save_results, ExportFormat,
    OutputFormat,
};

#[cfg(feature = "ai")]
use tracing::{error, info};

#[cfg(feature = "ai")]
use crate::domain::semantic_cleaner::SemanticCleaner;

#[cfg(feature = "ai")]
use crate::domain::DocumentChunk;

#[cfg(feature = "ai")]
use crate::error::SemanticError;

#[cfg(feature = "ai")]
use crate::error::ErrorClass;

// ============================================================================
// Export Results (RAG pipeline)
// ============================================================================

/// Configuration for the export flow.
#[allow(dead_code)]
pub struct ExportConfig<'a> {
    pub(crate) results: &'a [ScrapedContent],
    pub(crate) output_dir: PathBuf,
    pub(crate) format: OutputFormat,
    pub(crate) export_format: ExportFormat,
    pub(crate) clean_ai: bool,
    pub(crate) quick_save: bool,
    pub(crate) vault_path: Option<&'a PathBuf>,
    pub(crate) obsidian_options: ObsidianOptions,
    pub(crate) state_store: Option<&'a StateStore>,
    pub(crate) resume: bool,
    /// AI settings (only used when clean_ai is true and feature is enabled)
    pub(crate) ai_threshold: f32,
    pub(crate) ai_max_tokens: usize,
    pub(crate) ai_offline: bool,
    pub(crate) ai_model: String,
}

/// Run the export flow: AI-cleaned or standard export.
///
/// Returns the list of processed URLs on success.
#[cfg(feature = "ai")]
pub async fn run_export(
    config: ExportConfig<'_>,
    ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
) -> Result<Vec<String>, CliExit> {
    if config.clean_ai {
        match ai_cleaner {
            Some(cleaner) => run_ai_export(&config, cleaner).await,
            None => Err(CliExit::ConfigError(
                "Se solicitó limpieza semántica AI pero el limpiador no está disponible (no se propagó una falla de inicialización)"
                    .into(),
            )),
        }
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
            "AI semantic cleaning requires --features ai. Recompile with: cargo run --features ai"
                .into(),
        ));
    }
    run_standard_export(&config)
}

/// Standard export path (backward compatible).
fn run_standard_export(config: &ExportConfig<'_>) -> Result<Vec<String>, CliExit> {
    // Bridge the legacy StateStore handle onto the v2 RecordStore seam:
    // same directory + domain, so a legacy v1 state file migrates in place
    // on first load (Gate 2 policy lives inside RecordStore).
    let record_store = config.state_store.map(|ss| {
        let path = ss.get_state_path();
        let dir = path
            .parent()
            .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf);
        let domain = path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("unknown")
            .to_string();
        crate::infrastructure::export::RecordStore::new(domain).with_state_dir(dir)
    });
    let ctx = record_store
        .as_ref()
        .map(|store| export_factory::ResumeContext::new(store).with_resume(config.resume));
    match export_factory::process_results(
        config.results,
        config.output_dir.clone(),
        config.export_format,
        "export",
        ctx.as_ref(),
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
async fn run_ai_export(
    config: &ExportConfig<'_>,
    cleaner: std::sync::Arc<dyn SemanticCleaner>,
) -> Result<Vec<String>, CliExit> {
    info!(
        "Starting AI cleaning for {} pages concurrently...",
        config.results.len()
    );

    let cleaned_chunks = clean_all_pages(config.results, &cleaner).await?;

    info!(
        "AI cleaning complete: {} chunks from {} pages",
        cleaned_chunks.len(),
        config.results.len()
    );

    let record_store = config.state_store.map(|ss| {
        let path = ss.get_state_path();
        let dir = path
            .parent()
            .map_or_else(|| PathBuf::from("."), std::path::Path::to_path_buf);
        let domain = path
            .file_stem()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("unknown")
            .to_string();
        crate::infrastructure::export::RecordStore::new(domain).with_state_dir(dir)
    });
    let ctx = record_store
        .as_ref()
        .map(|store| export_factory::ResumeContext::new(store).with_resume(config.resume));

    match export_factory::process_results_with_chunks(
        &cleaned_chunks,
        config.output_dir.clone(),
        config.export_format,
        "export",
        ctx.as_ref(),
    ) {
        Ok(urls) => Ok(urls),
        Err(e) => {
            warn!("Failed to export cleaned results: {}", e);
            Err(CliExit::IoError(e.to_string()))
        },
    }
}

/// Run the cleaner over every scraped page concurrently and fold the results
/// into a single chunk list.
///
/// Per-page failures fall back to the raw content, but a TOTAL failure (every
/// page fails to clean) is treated as a model/config error and propagated as
/// `Err` instead of silently exiting 0 (#543).
#[cfg(feature = "ai")]
async fn clean_all_pages(
    results: &[ScrapedContent],
    cleaner: &std::sync::Arc<dyn SemanticCleaner>,
) -> Result<Vec<DocumentChunk>, CliExit> {
    let cleaning_tasks: Vec<_> = results
        .iter()
        .map(|result| {
            let html_content = result
                .html
                .clone()
                .unwrap_or_else(|| result.content.clone());
            let url = result.url.clone();
            let cleaner = std::sync::Arc::clone(cleaner);
            async move {
                let chunks_result = cleaner.clean(url.as_str(), &html_content).await;
                (url, chunks_result, result.clone())
            }
        })
        .collect();

    let cleaning_results = futures::future::join_all(cleaning_tasks).await;

    let mut cleaned_chunks: Vec<DocumentChunk> = Vec::with_capacity(results.len() * 2);
    let mut failed = 0usize;
    let mut fallback = 0usize;
    let mut first_error: Option<SemanticError> = None;
    for (url, chunks_result, result) in cleaning_results {
        match chunks_result {
            Ok(chunks) => {
                if chunks.is_empty() {
                    warn!("AI cleaner produced 0 chunks for: {}", url);
                    cleaned_chunks.push(DocumentChunk::from_scraped_content(&result));
                } else {
                    // The cleaner produces chunks with empty url/title (it only
                    // sees raw HTML). Enrich each chunk with identity from its
                    // source page so validate() passes and export succeeds (#569).
                    cleaned_chunks.extend(
                        chunks
                            .into_iter()
                            .map(|chunk| chunk.enrich_from_scraped_content(&result)),
                    );
                }
            },
            Err(e) => {
                // Classify the error by operational severity to decide
                // fail-fast vs fallback (#581 follow-up: error classification).
                match e.classify() {
                    ErrorClass::InternalFatal => {
                        // The model/inference stack is broken — retrying won't
                        // help, so abort the whole crawl immediately instead of
                        // burning CPU on 100 more pages that will all fail.
                        return Err(CliExit::ConfigError(format!(
                            "Falló la infraestructura de IA (error fatal, no reintentable): {e}"
                        )));
                    },
                    ErrorClass::DomainRecoverable => {
                        // ChunkTooLarge and similar: the chunk simply exceeds
                        // the user's --max-tokens limit. Fall back to raw for
                        // this page and count it as a fallback (so an all-
                        // fallback job still surfaces an error, #543).
                        warn!(
                            "Chunk excede límite para {}, usando contenido raw: {}",
                            url, e
                        );
                        cleaned_chunks.push(DocumentChunk::from_scraped_content(&result));
                        fallback += 1;
                        if first_error.is_none() {
                            first_error = Some(e);
                        }
                        continue;
                    },
                    // SemanticError::classify() only returns InternalFatal or
                    // DomainRecoverable, but handle the other classes
                    // exhaustively so future variants don't silently fall through.
                    ErrorClass::TransientRetriable
                    | ErrorClass::TransientBackoff
                    | ErrorClass::PermanentFatal => {
                        failed += 1;
                        error!("Falló limpieza de contenido para {}: {}", url, e);
                        if first_error.is_none() {
                            first_error = Some(e);
                        }
                        cleaned_chunks.push(DocumentChunk::from_scraped_content(&result));
                    },
                }
            },
        }
    }

    // If EVERY page failed or fell back, the cause is systemic (not per-
    // content), so propagate it instead of returning raw fallback chunks
    // with a success exit code (#543 regression).
    if failed + fallback == results.len() && !results.is_empty() {
        let detail = first_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "sin detalle de error disponible".to_string());
        return Err(CliExit::ConfigError(format!(
            "Falló la limpieza semántica AI en todas las páginas (error de modelo/configuración): {detail}"
        )));
    }

    Ok(cleaned_chunks)
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
    output_dir: &Path,
    format: &OutputFormat,
    obsidian_options: &ObsidianOptions,
) {
    if let Err(e) = save_results(results, output_dir, format, obsidian_options) {
        warn!("Failed to save individual files: {}", e);
        // Continue — file save is non-fatal, RAG export succeeded
    }
}

#[cfg(all(test, feature = "ai"))]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use crate::cli::error::CliExit;
    use crate::domain::semantic_cleaner::{private::Sealed, SemanticCleaner};
    use crate::domain::value_objects::ValidUrl;
    use crate::domain::DocumentChunk;
    use crate::domain::ScrapedContent;
    use crate::error::SemanticError;

    use super::clean_all_pages;

    /// Mock cleaner that always fails — used to assert total-failure propagation.
    struct FailingCleaner;

    impl Sealed for FailingCleaner {}

    impl SemanticCleaner for FailingCleaner {
        fn clean<'a>(
            &'a self,
            _url: &'a str,
            _html: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>>
        {
            Box::pin(async move {
                Err(SemanticError::Inference(
                    "mock cleaner always fails (#543 regression)".into(),
                ))
            })
        }

        fn max_tokens(&self) -> usize {
            512
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    fn sample_result(url: &str, html: &str) -> ScrapedContent {
        ScrapedContent {
            title: String::new(),
            content: String::new(),
            url: ValidUrl::parse(url).expect("valid test url"),
            excerpt: None,
            author: None,
            date: None,
            html: Some(html.to_string()),
            assets: Vec::new(),
            correlation_id: None,
            quality_hint: None,
        }
    }

    /// #543 regression: when EVERY page fails to clean, the pipeline must return
    /// an error (non-zero exit) instead of silently exiting 0 with raw fallback.
    ///
    /// With the ErrorClass fail-fast design (#581 follow-up), an infrastructure-
    /// fatal error (Inference/ModelLoad) aborts the whole crawl on the FIRST
    /// page instead of processing all pages just to discover they all fail.
    #[tokio::test]
    async fn test_clean_all_pages_total_failure_propagates_error() {
        let cleaner: Arc<dyn SemanticCleaner> = Arc::new(FailingCleaner);
        let results = vec![
            sample_result("https://example.com/a", "<p>a</p>"),
            sample_result("https://example.com/b", "<p>b</p>"),
        ];

        let outcome = clean_all_pages(&results, &cleaner).await;

        // Inference error is InternalFatal → fail-fast: abort immediately with
        // a config error instead of waiting for all pages to fail.
        match outcome {
            Err(CliExit::ConfigError(msg)) => {
                assert!(
                    msg.contains("fatal") || msg.contains("infraestructura"),
                    "error should indicate infrastructure failure: {msg}"
                );
            },
            other => panic!(
                "expected Err(CliExit::ConfigError) for infrastructure failure, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }
}
