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
    /// Download images and documents from HTML into the output directory.
    ///
    /// The boolean toggles `images` (default: true) and `documents`
    /// (default: false) select which asset kinds are downloaded. Files are
    /// written with SHA-256 hashed filenames; the response reports each
    /// downloaded asset with its local path.
    #[tool(
        description = "Download images (default: true) and/or documents (default: false) referenced in HTML content into the output directory (SHA-256 hashed filenames). 'images' and 'documents' are boolean toggles, not URL lists. Returns the downloaded assets with their local paths. For a full scrape that also downloads assets, use scrape_with_options with download_images/download_documents."
    )]
    #[instrument(skip(self), fields(base_url = %params.base_url, images = params.images.unwrap_or(true), documents = params.documents.unwrap_or(false), output_dir = ?params.output_dir))]
    async fn download_assets(
        &self,
        Parameters(params): Parameters<DownloadAssetsParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, assets);

        let base_url = url::Url::parse(&params.base_url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid base URL: {e}"),
                Some(serde_json::Value::String("base_url".to_string())),
            )
        })?;

        let mut config = webfang_core::infrastructure::config::ScraperConfig {
            download_images: params.images.unwrap_or(true),
            download_documents: params.documents.unwrap_or(false),
            ..Default::default()
        };
        if let Some(ref output_dir) = params.output_dir {
            config.output_dir = std::path::PathBuf::from(output_dir);
        }

        // Shared downloader when injected (connection pooling across calls);
        // None falls back to a per-call Downloader built from the config.
        let dl = self
            .state
            .downloader
            .as_deref()
            .map(|d| d as &dyn webfang_core::domain::ports::AssetDownloaderPort);

        match webfang_core::application::scraper_service::download_assets_if_enabled(
            &params.html,
            &base_url,
            &config,
            dl,
        )
        .await
        {
            Ok(assets) => {
                tracing::info!(
                    downloaded = assets.len(),
                    images = config.download_images,
                    documents = config.download_documents,
                    "assets downloaded"
                );
                let content = serde_json::to_string_pretty(&assets)
                    .unwrap_or_else(|_| "failed to serialize".into());
                Ok(CallToolResult::success(vec![Content::text(content)]))
            },
            Err(e) => Ok(CallToolResult::error(vec![Content::text(e.to_string())])),
        }
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_assets()
}
