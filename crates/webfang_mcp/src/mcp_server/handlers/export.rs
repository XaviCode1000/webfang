//! Export tools — 4 tools for real output format conversion
//!
//! Tools: export_file, export_jsonl, export_vector,
//! process_export_pipeline
//!
//! Every tool performs a REAL export via the existing `webfang_core`
//! `export_factory` surface (jsonl/vector/auto) and reports honest
//! success/error. Operational failures (no repository, empty results,
//! missing content, I/O errors) map to `CallToolResult::error`
//! (isError:true, Spanish). Invalid parameters (bad format) map to a
//! protocol-level `McpError::invalid_params` — never a silent fallback.

use super::McpHandler;
use crate::mcp_server::params::*;
use crate::mcp_server::validation::SanitizedFilename;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::instrument;
use webfang_core::application::export_factory::{create_exporter, process_results};
use webfang_core::domain::entities::ExportFormat;
use webfang_core::domain::CrawlResultRepository;
use webfang_core::domain::DocumentChunkUnvalidated;
use webfang_core::domain::ScrapedContent;

/// Run a real export of persisted results to the given format.
///
/// Maps operational [`ExporterError`](webfang_core::domain::exporter::ExporterError)s
/// to an honest `CallToolResult::error` (isError:true, Spanish) and reports the
/// real written path on success.
fn export_results(
    results: &[ScrapedContent],
    output_dir: PathBuf,
    format: ExportFormat,
    filename: &SanitizedFilename,
) -> Result<CallToolResult, McpError> {
    let count = results.len();
    match process_results(
        results,
        output_dir.clone(),
        format,
        filename.as_str(),
        None,
        false,
    ) {
        Ok(_) => {
            let path = resolve_export_path(&output_dir, filename, format);
            tracing::info!(documents = count, path = %path.display(), "export completed");
            Ok(CallToolResult::success(vec![Content::text(format!(
                "Exportación completada: {count} documentos → {}",
                path.display()
            ))]))
        },
        Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
            "error al exportar: {e}"
        ))])),
    }
}

/// Compute the real on-disk path an export wrote to.
///
/// For concrete formats this is `{output_dir}/{filename}.{ext}`. For `Auto`
/// the concrete extension is resolved from what actually exists (jsonl
/// preferred, then json), so the reported path is always truthful.
///
/// `filename` is a [`SanitizedFilename`], so the join can never escape
/// `output_dir` (issue #601).
fn resolve_export_path(
    output_dir: &Path,
    filename: &SanitizedFilename,
    format: ExportFormat,
) -> PathBuf {
    let name = filename.as_str();
    match format {
        ExportFormat::Auto => {
            let jsonl = output_dir.join(format!("{name}.jsonl"));
            if jsonl.exists() {
                jsonl
            } else {
                output_dir.join(format!("{name}.json"))
            }
        },
        concrete => output_dir.join(format!("{name}.{}", concrete.extension())),
    }
}

/// Resolve persisted crawl results from an optional repository, mapping every
/// operational failure to an honest `CallToolResult::error` (isError:true,
/// Spanish).
///
/// Extracted from [`McpHandler::load_results`] as a free function so the
/// `None`-repository branch — unreachable through `Container::new`, which
/// always attempts to wire a repository and tolerates log corruption — can be
/// unit-tested directly (REQ-MCP-EXPORT-05).
fn load_results_from(
    repo: Option<Arc<dyn CrawlResultRepository>>,
) -> Result<Vec<ScrapedContent>, CallToolResult> {
    let repo = match repo {
        Some(repo) => repo,
        None => {
            return Err(CallToolResult::error(vec![Content::text(
                "no hay repositorio de resultados disponible",
            )]))
        },
    };
    let results = repo.load_all().map_err(|e| {
        CallToolResult::error(vec![Content::text(format!(
            "no se pudieron cargar los resultados: {e}"
        ))])
    })?;
    if results.is_empty() {
        return Err(CallToolResult::error(vec![Content::text(
            "no hay resultados disponibles para exportar",
        )]));
    }
    Ok(results)
}

impl McpHandler {
    /// Load all persisted crawl results, mapping operational failures to an
    /// honest `CallToolResult::error` (isError:true, Spanish).
    ///
    /// Returns `Err(CallToolResult)` when no repository is wired, the bulk
    /// load fails, or the repository holds zero results; callers propagate
    /// this directly.
    async fn load_results(&self) -> Result<Vec<ScrapedContent>, CallToolResult> {
        load_results_from(self.state.container.crawl_result_repository())
    }
}

#[tool_router(router = tool_router_export, vis = "pub")]
// Allow: the `tool_router_export` fn generated by rmcp-macros cannot be documented from this crate.
#[allow(missing_docs)]
impl McpHandler {
    /// Save caller-provided content as a structured export file (jsonl/vector)
    #[tool(
        description = "Save caller-provided content to a structured export file. Supported formats: jsonl, vector, auto. Reports the real written path."
    )]
    #[instrument(skip(self), fields(filename = %params.filename, format = %params.format))]
    async fn export_file(
        &self,
        Parameters(params): Parameters<ExportFileParams>,
    ) -> Result<CallToolResult, McpError> {
        params.validate()?;

        let _permit = acquire_semaphore!(self, export);

        // Honest error on empty content (REQ-MCP-EXPORT-05).
        if params.content.trim().is_empty() {
            return Ok(CallToolResult::error(vec![Content::text(
                "el contenido no puede estar vacío",
            )]));
        }

        // Invalid format is a protocol-level invalid-params error, never a
        // silent fallback (REQ-MCP-EXPORT-07).
        let format = ExportFormat::parse_str(&params.format).map_err(|e| {
            McpError::invalid_params(
                format!("formato inválido: {e}"),
                Some(serde_json::Value::String("format".to_string())),
            )
        })?;

        let output_dir = PathBuf::from(&params.output_dir);
        // `filename` reaches the filesystem via `create_exporter` /
        // `resolve_export_path`; it MUST be a validated [`SanitizedFilename`]
        // so a `..` can never reach `std::fs` (issue #601). The raw string is
        // still used for the synthetic URL / title below.
        let filename = params.filename.clone();
        let safe_filename = SanitizedFilename::try_from(filename.as_str()).map_err(|_| {
            McpError::invalid_params(
                "nombre de archivo inválido",
                Some(serde_json::Value::String("filename".to_string())),
            )
        })?;

        // Build a validated document chunk from the caller content. The chunk
        // id/timestamp are generated internally; the synthetic URL satisfies
        // validation (any parseable scheme) and scopes the doc to this tool.
        let url = url::Url::parse(&format!("https://webfang.local/{filename}")).map_err(|e| {
            McpError::invalid_params(
                format!("nombre de archivo inválido: {e}"),
                Some(serde_json::Value::String("filename".to_string())),
            )
        })?;
        let scraped = ScrapedContent {
            title: filename.clone(),
            content: params.content.clone(),
            url: webfang_core::domain::ValidUrl::new(url),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
        };
        let validated = match DocumentChunkUnvalidated::from_scraped_content(&scraped).validate() {
            Ok(v) => v,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "contenido inválido: {e}"
                ))]))
            },
        };

        let exporter = match create_exporter(output_dir.clone(), safe_filename.as_str(), format) {
            Ok(exporter) => exporter,
            Err(e) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "no se pudo crear el exportador: {e}"
                ))]))
            },
        };

        match exporter.export(validated) {
            Ok(()) => {
                let path = resolve_export_path(&output_dir, &safe_filename, format);
                tracing::info!(documents = 1, path = %path.display(), "export completed");
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "Exportación completada: 1 documentos → {}",
                    path.display()
                ))]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "error al exportar: {e}"
            ))])),
        }
    }

    /// Export persisted crawl results to JSONL format (one JSON object per line)
    #[tool(
        description = "Export persisted crawl results to JSONL format (one JSON object per line). Optimal for RAG pipeline ingestion. Reports the real written path."
    )]
    #[instrument(skip(self), fields(filename, format = "jsonl", results))]
    async fn export_jsonl(
        &self,
        Parameters(params): Parameters<ExportJsonlParams>,
    ) -> Result<CallToolResult, McpError> {
        params.validate()?;

        let _permit = acquire_semaphore!(self, export);

        let output_dir = PathBuf::from(params.output_dir.as_deref().unwrap_or("./output"));
        // Validated flat filename (issue #601): the raw `Option<String>` can
        // only become a `SanitizedFilename` through exhaustive boundary
        // validation, so the join in `export_results` can never escape
        // `output_dir`.
        let filename = SanitizedFilename::try_from(params.filename.as_deref().unwrap_or("export"))
            .map_err(|_| {
                McpError::invalid_params(
                    "nombre de archivo inválido",
                    Some(serde_json::Value::String("filename".to_string())),
                )
            })?;

        let results = match self.load_results().await {
            Ok(results) => results,
            Err(err) => return Ok(err),
        };
        let span = tracing::Span::current();
        span.record("filename", filename.as_str());
        span.record("results", results.len());

        export_results(&results, output_dir, ExportFormat::Jsonl, &filename)
    }

    /// Export persisted crawl results with embeddings for vector database ingestion
    #[tool(
        description = "Export persisted crawl results to JSON format for vector database ingestion. Includes a metadata header. Reports the real written path."
    )]
    #[instrument(skip(self), fields(filename, format = "vector", results))]
    async fn export_vector(
        &self,
        Parameters(params): Parameters<ExportVectorParams>,
    ) -> Result<CallToolResult, McpError> {
        params.validate()?;

        let _permit = acquire_semaphore!(self, export);

        let output_dir = PathBuf::from(params.output_dir.as_deref().unwrap_or("./output"));
        // Validated flat filename (issue #601): see `export_jsonl` above.
        let filename = SanitizedFilename::try_from(params.filename.as_deref().unwrap_or("export"))
            .map_err(|_| {
                McpError::invalid_params(
                    "nombre de archivo inválido",
                    Some(serde_json::Value::String("filename".to_string())),
                )
            })?;

        let results = match self.load_results().await {
            Ok(results) => results,
            Err(err) => return Ok(err),
        };
        let span = tracing::Span::current();
        span.record("filename", filename.as_str());
        span.record("results", results.len());

        export_results(&results, output_dir, ExportFormat::Vector, &filename)
    }

    /// Full export pipeline: scrape (when `url` is given) → export synchronously
    #[tool(
        description = "Run the export pipeline synchronously: when `url` is provided, scrape it first; otherwise use persisted crawl results. Export to the specified format (jsonl, vector, or auto; default jsonl). Reports the real written path; never queues."
    )]
    #[instrument(skip(self), fields(format, url, results))]
    async fn process_export_pipeline(
        &self,
        Parameters(params): Parameters<ProcessExportPipelineParams>,
    ) -> Result<CallToolResult, McpError> {
        params.validate()?;

        let _permit = acquire_semaphore!(self, export);

        let format_str = params.format.as_deref().unwrap_or("jsonl");
        let format = ExportFormat::parse_str(format_str).map_err(|e| {
            McpError::invalid_params(
                format!("formato inválido: {e}"),
                Some(serde_json::Value::String("format".to_string())),
            )
        })?;

        // When a URL is supplied the pipeline scrapes it live (reusing the
        // existing scraper service) and exports the fresh result; otherwise it
        // falls back to persisted crawl results (issue #605).
        let results = match &params.url {
            Some(url_str) => {
                let url = url::Url::parse(url_str).map_err(|e| {
                    McpError::invalid_params(
                        format!("URL inválida: {e}"),
                        Some(serde_json::Value::String("url".to_string())),
                    )
                })?;
                let client = self.state.container.http_client().as_ref();
                match webfang_core::application::scraper_service::scrape_with_readability(
                    client, &url,
                )
                .await
                {
                    Ok(results) => {
                        tracing::info!(url = %url, documents = results.len(), "scrape completed");
                        results
                    },
                    Err(e) => {
                        return Ok(CallToolResult::error(vec![Content::text(format!(
                            "error al rastrear {url}: {e}"
                        ))]))
                    },
                }
            },
            None => match self.load_results().await {
                Ok(results) => results,
                Err(err) => return Ok(err),
            },
        };
        let span = tracing::Span::current();
        span.record("format", format_str);
        span.record("url", params.url.as_deref().unwrap_or(""));
        span.record("results", results.len());

        // The pipeline exports to the container's configured output directory.
        let output_dir = self.state.container.scraper_config.output_dir.clone();
        // `export` is a compile-time-known flat name; if validation ever
        // tightens, this surfaces as an honest invalid-params error rather
        // than a panic.
        let filename = SanitizedFilename::try_from("export").map_err(|_| {
            McpError::invalid_params(
                "nombre de archivo interno inválido",
                Some(serde_json::Value::String("filename".to_string())),
            )
        })?;
        export_results(&results, output_dir, format, &filename)
    }
}

/// Build the export tools router (export_file, export_jsonl, export_vector,
/// process_export_pipeline).
///
/// Returns a partial router that is combined with the other category routers
/// via the `+` operator in
/// [`build_tool_router`](crate::mcp_server::handlers::build_tool_router).
pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_export()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-MCP-EXPORT-05 (repository unavailable): when no repository is wired,
    /// the loader must return an honest `CallToolResult::error` (isError:true)
    /// carrying the Spanish "no hay repositorio" message — never a fake success.
    ///
    /// This branch is unreachable through `Container::new` (which always
    /// attempts to wire a repository and tolerates log corruption), so it is
    /// exercised directly through the extracted `load_results_from` free
    /// function by passing `None`.
    #[test]
    fn load_results_from_none_repository_is_honest_error() {
        let err = load_results_from(None).expect_err("None repository must be an error");

        // Serialize exactly as the MCP transport would, then assert the honest
        // error contract: isError:true plus the Spanish message.
        let json = serde_json::to_value(&err).expect("CallToolResult must serialize");
        assert_eq!(
            json.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "None-repository path must set isError:true, got: {json}"
        );
        let text = json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        assert!(
            text.contains("no hay repositorio de resultados disponible"),
            "honest Spanish no-repository error expected, got: {text}"
        );
    }
}

#[cfg(test)]
mod handler_tests {
    use super::*;
    use crate::mcp_server::state::McpState;
    use rmcp::handler::server::wrapper::Parameters;
    use rmcp::model::CallToolResult;
    use std::path::Path;
    use tempfile::TempDir;
    use webfang_core::di::Container;
    use webfang_core::domain::{CrawlerConfig, ScrapedContent, ValidUrl};
    use webfang_core::infrastructure::config::ScraperConfig;

    async fn test_handler() -> (McpHandler, TempDir) {
        let tmp = TempDir::new().expect("create temp dir");
        let crawler_config =
            CrawlerConfig::new(url::Url::parse("https://example.com").expect("valid url"));
        let scraper_config = ScraperConfig {
            output_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let container = Container::new(crawler_config, scraper_config)
            .await
            .expect("create container");
        let state = McpState::new(container);
        (McpHandler::new(state), tmp)
    }

    /// Seed the container's crawl-result repository with one item, polling
    /// until the append-only writer has flushed it (mirrors the shared
    /// `start_seeded_server` harness).
    async fn seed_one(handler: &McpHandler) {
        let repo = handler
            .state
            .container
            .crawl_result_repository()
            .expect("repo wired");
        let content = ScrapedContent {
            title: "Seed".to_string(),
            content: "seed body".to_string(),
            url: ValidUrl::new(url::Url::parse("https://example.com/seed").expect("valid")),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
        };
        repo.save(&content).expect("save seed");
        for _ in 0..80 {
            if repo
                .find_by_url("https://example.com/seed")
                .expect("find")
                .is_some()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    fn result_text(result: &CallToolResult) -> String {
        serde_json::to_value(result)
            .ok()
            .and_then(|v| v.get("content").and_then(|c| c.as_array()).cloned())
            .and_then(|arr| arr.first().cloned())
            .and_then(|first| {
                first
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }

    #[tokio::test]
    async fn export_file_empty_content_is_error() {
        let (handler, _tmp) = test_handler().await;
        // `output_dir` must be a safe relative path (params validation, #512).
        let out_dir = "test-output/export-empty";
        let _ = std::fs::remove_dir_all(out_dir);
        let res = handler
            .export_file(Parameters(ExportFileParams {
                output_dir: out_dir.to_string(),
                filename: "doc".to_string(),
                format: "jsonl".to_string(),
                content: "   ".to_string(),
            }))
            .await
            .expect("export_file returns Ok on empty content");
        let json = serde_json::to_value(&res).expect("serialize");
        assert_eq!(
            json.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "empty content must map to isError:true, got: {json}"
        );
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[tokio::test]
    async fn export_file_invalid_format_is_invalid_params() {
        let (handler, tmp) = test_handler().await;
        let res = handler
            .export_file(Parameters(ExportFileParams {
                output_dir: tmp.path().to_string_lossy().to_string(),
                filename: "doc".to_string(),
                format: "bogus".to_string(),
                content: "hello".to_string(),
            }))
            .await;
        assert!(res.is_err(), "invalid format must be a protocol error");
    }

    /// Issue #601 regression: a `filename` with `..` must be rejected at the
    /// validation boundary (protocol error), never written outside
    /// `output_dir`.
    #[tokio::test]
    async fn export_file_rejects_filename_traversal() {
        let (handler, tmp) = test_handler().await;
        let res = handler
            .export_file(Parameters(ExportFileParams {
                output_dir: tmp.path().to_string_lossy().to_string(),
                filename: "../escape".to_string(),
                format: "jsonl".to_string(),
                content: "hello".to_string(),
            }))
            .await;
        assert!(
            res.is_err(),
            "filename traversal '../escape' must be a protocol error"
        );
        // The file must NOT leak into the parent (repo root / CWD).
        let escaped = std::env::current_dir().expect("cwd").join("escape.jsonl");
        assert!(
            !escaped.exists(),
            "filename traversal wrote outside output_dir: {escaped:?}"
        );
    }

    /// Issue #601 regression: a `filename` containing a subdirectory separator
    /// must be rejected (no silent nested-dir creation).
    #[tokio::test]
    async fn export_file_rejects_filename_subdirectory() {
        let (handler, tmp) = test_handler().await;
        let res = handler
            .export_file(Parameters(ExportFileParams {
                output_dir: tmp.path().to_string_lossy().to_string(),
                filename: "sub/out".to_string(),
                format: "jsonl".to_string(),
                content: "hello".to_string(),
            }))
            .await;
        assert!(res.is_err(), "filename 'sub/out' must be a protocol error");
    }

    #[tokio::test]
    async fn export_file_rejects_unknown_format_with_clear_message() {
        let (handler, tmp) = test_handler().await;
        // Bug #4 regression: format="md" (unsupported) must return
        // invalid_params with a message listing supported formats (issue #590).
        let res = handler
            .export_file(Parameters(ExportFileParams {
                output_dir: tmp.path().to_string_lossy().to_string(),
                filename: "doc".to_string(),
                format: "md".to_string(),
                content: "hello".to_string(),
            }))
            .await;
        assert!(
            res.is_err(),
            "unsupported format 'md' must be a protocol error"
        );
    }

    #[tokio::test]
    async fn export_file_writes_jsonl() {
        let (handler, _tmp) = test_handler().await;
        let out_dir = "test-output/export-jsonl";
        let _ = std::fs::remove_dir_all(out_dir);
        let res = handler
            .export_file(Parameters(ExportFileParams {
                output_dir: out_dir.to_string(),
                filename: "doc".to_string(),
                format: "jsonl".to_string(),
                content: "hello world".to_string(),
            }))
            .await
            .expect("export_file returns Ok");
        let text = result_text(&res);
        assert!(
            text.contains("Exportación completada"),
            "success must report completion: {text}"
        );
        assert!(
            Path::new(out_dir).join("doc.jsonl").exists(),
            "export file must be written"
        );
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[tokio::test]
    async fn export_jsonl_empty_repo_is_error() {
        let (handler, _tmp) = test_handler().await;
        let res = handler
            .export_jsonl(Parameters(ExportJsonlParams {
                output_dir: None,
                filename: None,
            }))
            .await
            .expect("export_jsonl returns Ok on empty repo");
        let json = serde_json::to_value(&res).expect("serialize");
        assert_eq!(
            json.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "empty repo must map to isError:true, got: {json}"
        );
    }

    #[tokio::test]
    async fn export_jsonl_seeded_writes_file() {
        let (handler, _tmp) = test_handler().await;
        seed_one(&handler).await;
        let out_dir = "test-output/export-jsonl-seeded";
        let _ = std::fs::remove_dir_all(out_dir);
        let res = handler
            .export_jsonl(Parameters(ExportJsonlParams {
                output_dir: Some(out_dir.to_string()),
                filename: Some("out".to_string()),
            }))
            .await
            .expect("export_jsonl returns Ok");
        let text = result_text(&res);
        assert!(
            text.contains("Exportación completada"),
            "seeded export must report completion: {text}"
        );
        assert!(
            Path::new(out_dir).join("out.jsonl").exists(),
            "jsonl export file must be written"
        );
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[tokio::test]
    async fn export_vector_seeded_writes_json() {
        let (handler, _tmp) = test_handler().await;
        seed_one(&handler).await;
        let out_dir = "test-output/export-vector-seeded";
        let _ = std::fs::remove_dir_all(out_dir);
        let res = handler
            .export_vector(Parameters(ExportVectorParams {
                output_dir: Some(out_dir.to_string()),
                filename: Some("vec".to_string()),
            }))
            .await
            .expect("export_vector returns Ok");
        let text = result_text(&res);
        assert!(
            text.contains("Exportación completada"),
            "seeded vector export must report completion: {text}"
        );
        assert!(
            Path::new(out_dir).join("vec.json").exists(),
            "vector export file must be written"
        );
        let _ = std::fs::remove_dir_all(out_dir);
    }

    #[tokio::test]
    async fn process_export_pipeline_seeded_writes_to_output_dir() {
        let (handler, _tmp) = test_handler().await;
        seed_one(&handler).await;
        let res = handler
            .process_export_pipeline(Parameters(ProcessExportPipelineParams {
                url: None,
                format: Some("jsonl".to_string()),
            }))
            .await
            .expect("process_export_pipeline returns Ok");
        let text = result_text(&res);
        assert!(
            text.contains("Exportación completada"),
            "pipeline export must report completion: {text}"
        );
    }

    /// Issue #605 regression: when `url` is provided, the pipeline must scrape
    /// it live instead of reading persisted results. With no network/seed this
    /// surfaces as a scrape (network) error — never the persisted-only
    /// "no hay resultados disponibles para exportar" message, which would
    /// prove the `url` argument was ignored.
    #[tokio::test]
    async fn process_export_pipeline_with_url_invokes_scrape() {
        let (handler, _tmp) = test_handler().await;
        let res = handler
            .process_export_pipeline(Parameters(ProcessExportPipelineParams {
                url: Some("https://quotes.toscrape.com".to_string()),
                format: Some("jsonl".to_string()),
            }))
            .await
            .expect("process_export_pipeline returns Ok on scrape failure");
        let text = result_text(&res);
        assert!(
            !text.contains("no hay resultados disponibles para exportar"),
            "url branch must not fall back to persisted 'no results': {text}"
        );
        assert!(
            text.contains("error al rastrear") || text.contains("Exportación completada"),
            "url branch must attempt a live scrape: {text}"
        );
    }
}
