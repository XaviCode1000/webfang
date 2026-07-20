//! Asset Management tools — 1 tool for downloading images/documents
//!
//! Tools: download_assets

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use tracing::instrument;

#[tool_router(router = tool_router_assets, vis = "pub")]
impl McpHandler {
    /// Download images and documents from HTML
    #[tool(
        description = "Download images (PNG, JPG, GIF, WEBP, SVG, BMP) and/or documents (PDF, DOCX, XLSX, PPTX) from HTML content. Uses SHA256-hashed filenames."
    )]
    #[instrument(skip(self), fields(base_url = %params.base_url, images = params.images.unwrap_or(true), documents = params.documents.unwrap_or(false)))]
    async fn download_assets(
        &self,
        Parameters(params): Parameters<DownloadAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .assets
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let base_url = url::Url::parse(&params.base_url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid base URL: {e}"),
                Some(serde_json::Value::String("base_url".to_string())),
            )
        })?;

        let download_images = params.images.unwrap_or(true);
        let download_documents = params.documents.unwrap_or(false);

        // TODO(#170): Wire up actual asset downloading via shared Downloader
        // For now, return an explicit "not implemented" error instead of fake success
        Ok(CallToolResult::error(vec![Content::text(format!(
            "download_assets not yet implemented (see #170). \
             Use scrape_url with download_images=true instead. \
             Requested: images={download_images}, documents={download_documents}, base={base_url}"
        ))]))
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_assets()
}
