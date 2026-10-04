//! Export flow — handles result export (standard and AI-cleaned) and file saving.

use std::path::{Path, PathBuf};
use tracing::warn;

use crate::cli::error::CliExit;
use crate::cli::scrape_flow::{record_store_bridge, warn_unreadable_state};
use crate::domain::persistence::StateStorePort;
use crate::domain::ScrapedContent;
use crate::infrastructure::output::file_saver::ObsidianOptions;
use crate::{
    application::export_factory, infrastructure::output::file_saver::save_results, ExportFormat,
    OutputFormat,
};

#[cfg(feature = "ai")]
use tracing::info;

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
///
/// `results` is shared ownership (`Arc<[ScrapedContent]>`) rather than a
/// borrow: the export itself runs on the blocking pool (issue #1814, slice B
/// AC4), which requires `'static` data, while the caller (`orchestrator` /
/// `batch_flow`) keeps its own `Arc` handle for `report_phase` after the
/// export. Sharing the slice costs one refcount increment — never a clone of
/// page content.
#[allow(dead_code)]
pub struct ExportConfig<'a> {
    pub(crate) results: std::sync::Arc<[ScrapedContent]>,
    pub(crate) output_dir: PathBuf,
    pub(crate) format: OutputFormat,
    pub(crate) export_format: ExportFormat,
    pub(crate) clean_ai: bool,
    pub(crate) quick_save: bool,
    pub(crate) vault_path: Option<&'a PathBuf>,
    pub(crate) obsidian_options: ObsidianOptions,
    pub(crate) state_store: Option<&'a dyn StateStorePort>,
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
            Some(cleaner) => run_ai_export(config, cleaner).await,
            None => Err(CliExit::ConfigError(
                "Se solicitó limpieza semántica AI pero el limpiador no está disponible (no se propagó una falla de inicialización)"
                    .into(),
            )),
        }
    } else {
        run_standard_export(config).await
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
    run_standard_export(config).await
}

/// Surface the shared Spanish unreadable-state notice on the export resume
/// path (#1587): the export path previously resumed silently over unreadable
/// state while the scrape path warned. Single call site helper so both export
/// flows (standard + AI) stay identical without duplicating the bridge check.
fn warn_unreadable_resume_state(record_store: &Option<crate::infrastructure::export::RecordStore>) {
    if let Some(store) = record_store.as_ref() {
        warn_unreadable_state(store);
    }
}

/// Standard export path (backward compatible).
///
/// Issue #1814 (slice B, AC4): `process_results` is sync filesystem I/O plus a
/// SHA-256 per item plus resume-gate state writes, so it runs on the blocking
/// pool. The owned `RecordStore` and the shared `Arc<[ScrapedContent]>` move
/// into the closure — the D3 resume-gate ordering is untouched, only where the
/// work executes.
async fn run_standard_export(config: ExportConfig<'_>) -> Result<Vec<String>, CliExit> {
    // Bridge the state-store port onto the v2 RecordStore seam (shared
    // helper — same directory + domain derivation as the scrape path).
    // Built BEFORE the spawn: `config.state_store` is a borrow of caller
    // state, while the owned bridge is what moves into the blocking task.
    let record_store = config.state_store.map(record_store_bridge);
    warn_unreadable_resume_state(&record_store);
    let resume = config.resume;
    let output_dir = config.output_dir;
    let export_format = config.export_format;
    let results = config.results;
    let handle = tokio::task::spawn_blocking(move || {
        let ctx = record_store
            .as_ref()
            .map(|store| export_factory::ResumeContext::new(store).with_resume(resume));
        export_factory::process_results(&results, output_dir, export_format, "export", ctx.as_ref())
    });
    await_blocking_export(handle, "scrape", export_format).await
}

/// AI semantic cleaning export path.
#[cfg(feature = "ai")]
async fn run_ai_export(
    config: ExportConfig<'_>,
    cleaner: std::sync::Arc<dyn SemanticCleaner>,
) -> Result<Vec<String>, CliExit> {
    info!(
        "Starting AI cleaning for {} pages concurrently...",
        config.results.len()
    );

    // Surface the Spanish unreadable-state notice BEFORE the long AI run so a
    // degraded resume is visible up front rather than after minutes of
    // cleaning (#1587). Read-only: `load()` never mutates the state file.
    let record_store = config.state_store.map(record_store_bridge);
    warn_unreadable_resume_state(&record_store);

    let cleaned_chunks = clean_all_pages(&config.results, &cleaner).await?;

    info!(
        "AI cleaning complete: {} chunks from {} pages",
        cleaned_chunks.len(),
        config.results.len()
    );

    // Issue #1814 (slice B, AC4): `process_results_with_chunks` is sync
    // filesystem I/O + SHA-256 per chunk + resume-gate state writes — dispatch
    // it to the blocking pool with the owned chunks and record store. The
    // D3 ordering (claim/decide/commit/final_persist) is unchanged.
    let resume = config.resume;
    let output_dir = config.output_dir;
    let export_format = config.export_format;
    let handle = tokio::task::spawn_blocking(move || {
        let ctx = record_store
            .as_ref()
            .map(|store| export_factory::ResumeContext::new(store).with_resume(resume));
        export_factory::process_results_with_chunks(
            &cleaned_chunks,
            output_dir,
            export_format,
            "export",
            ctx.as_ref(),
        )
    });
    await_blocking_export(handle, "AI-cleaned", export_format).await
}

/// Await a blocking-pool export handle and translate its outcome onto the CLI
/// error surface.
///
/// Shared by the standard and AI export paths (issue #1814 slice B): the D3
/// sequence executes on the blocking pool; this wrapper only maps
/// `ExporterError` and `JoinError` onto `CliExit` with the Spanish
/// user-facing wording. Extracted so both `run_*_export` paths stay inside
/// the cognitive-complexity ratchet.
async fn await_blocking_export(
    handle: tokio::task::JoinHandle<Result<Vec<String>, crate::domain::exporter::ExporterError>>,
    stage: &'static str,
    format: ExportFormat,
) -> Result<Vec<String>, CliExit> {
    match handle.await {
        Ok(Ok(urls)) => Ok(urls),
        Ok(Err(e)) => {
            warn!(error = %e, format = ?format, "export of {stage} results failed");
            Err(CliExit::IoError(e.to_string()))
        },
        Err(join) => {
            warn!(stage, error = %join, format = ?format, "export task failed on the blocking pool");
            Err(CliExit::IoError(format!(
                "la exportación falló en el pool de bloqueo: {join}"
            )))
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
                    warn!(url = %url, "AI cleaner produced 0 chunks; using raw content fallback");
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
                            url = %url,
                            error = %e,
                            "chunk exceeds token limit; using raw content fallback"
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
                        crate::infrastructure::observability::log_scrape_error(
                            &e,
                            url.as_str(),
                            "ai_clean",
                            result.correlation_id.as_ref(),
                            "AI content cleanup failed; keeping raw content fallback",
                        );
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
        warn!(error = %e, "failed to save individual output files");
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

#[cfg(test)]
mod standard_export_dispatch {
    //! Behavior-preservation pins for the blocking-pool dispatch
    //! (issue #1814, slice B AC4): moving `process_results` onto the blocking
    //! pool must not change what the export writes or reports. The off-runtime
    //! scheduling property itself is proven by the starvation tests in
    //! `llm_wire.rs` and the MCP export handler suite.

    use std::sync::Arc;

    use super::{run_standard_export, ExportConfig};
    use crate::domain::value_objects::ValidUrl;
    use crate::domain::ScrapedContent;

    fn scraped(url: &str, title: &str, body: &str) -> ScrapedContent {
        ScrapedContent {
            title: title.to_string(),
            content: body.to_string(),
            url: ValidUrl::parse(url).expect("valid test url"),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
            quality_hint: None,
        }
    }

    fn config_for(
        results: Vec<ScrapedContent>,
        output_dir: std::path::PathBuf,
    ) -> ExportConfig<'static> {
        ExportConfig {
            results: Arc::from(results),
            output_dir,
            format: crate::OutputFormat::default(),
            export_format: crate::ExportFormat::Jsonl,
            clean_ai: false,
            quick_save: false,
            vault_path: None,
            obsidian_options: Default::default(),
            state_store: None,
            resume: false,
            ai_threshold: 0.5,
            ai_max_tokens: 512,
            ai_offline: false,
            ai_model: String::new(),
        }
    }

    #[tokio::test]
    async fn run_standard_export_writes_jsonl_and_reports_urls() {
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let config = config_for(
            vec![
                scraped("https://example.com/a", "A", "body a"),
                scraped("https://example.com/b", "B", "body b"),
            ],
            tmp.path().to_path_buf(),
        );

        let urls = run_standard_export(config).await.expect("export succeeds");

        assert_eq!(urls.len(), 2, "both items must be reported as processed");
        let body =
            std::fs::read_to_string(tmp.path().join("export.jsonl")).expect("jsonl is written");
        assert_eq!(
            body.lines().filter(|l| !l.trim().is_empty()).count(),
            2,
            "both items must reach the export file"
        );
    }
}
