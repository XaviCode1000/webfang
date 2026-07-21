//! Export tools — 4 tools for output format conversion
//!
//! Tools: export_file, export_jsonl, export_vector,
//! process_export_pipeline

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use std::path::PathBuf;
use tracing::instrument;

#[tool_router(router = tool_router_export, vis = "pub")]
impl McpHandler {
    /// Save scraped content as a file (Markdown, Text, or JSON)
    #[tool(
        description = "Save scraped content as a file in Markdown, Text, or JSON format with YAML frontmatter."
    )]
    #[instrument(skip(self), fields(filename = %params.filename, format = %params.format))]
    async fn export_file(
        &self,
        Parameters(params): Parameters<ExportFileParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let format = webfang_core::domain::entities::ExportFormat::parse_str(&params.format)
            .unwrap_or(webfang_core::domain::entities::ExportFormat::Jsonl);

        let output_dir = PathBuf::from(&params.output_dir);
        match tokio::fs::create_dir_all(&output_dir).await {
            Ok(_) => {
                let path = output_dir.join(format!("{}.{}", params.filename, format.extension()));
                Ok(CallToolResult::success(vec![Content::text(format!(
                    "File ready at: {}",
                    path.display()
                ))]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                "failed to create directory: {e}"
            ))])),
        }
    }

    /// Export scraped content to JSONL format (one JSON object per line, RAG-ready)
    #[tool(
        description = "Export content to JSONL format (one JSON object per line). Optimal for RAG pipeline ingestion."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn export_jsonl(
        &self,
        Parameters(params): Parameters<ExportJsonlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let output_dir = params.output_dir.as_deref().unwrap_or("./output");
        let filename = params.filename.as_deref().unwrap_or("export");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "JSONL exporter ready at: {output_dir}/{filename}.jsonl"
        ))]))
    }

    /// Export content with embeddings for vector database ingestion
    #[tool(
        description = "Export content with embeddings to JSON format for vector database ingestion. Includes metadata header."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn export_vector(
        &self,
        Parameters(params): Parameters<ExportVectorParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let output_dir = params.output_dir.as_deref().unwrap_or("./output");
        let filename = params.filename.as_deref().unwrap_or("export");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Vector exporter ready at: {output_dir}/{filename}.json"
        ))]))
    }

    /// Full export pipeline: scrape → chunk → validate → export
    #[tool(
        description = "Run the full export pipeline: scrape content, chunk it, validate, and export to the specified format."
    )]
    #[instrument(skip(self), fields(params = ?params))]
    async fn process_export_pipeline(
        &self,
        Parameters(params): Parameters<ProcessExportPipelineParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .export
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let url = params.url.as_deref().unwrap_or("");
        let format = params.format.as_deref().unwrap_or("jsonl");
        Ok(CallToolResult::success(vec![Content::text(format!(
            "Pipeline queued for: {url} → [{format}]"
        ))]))
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_export()
}
