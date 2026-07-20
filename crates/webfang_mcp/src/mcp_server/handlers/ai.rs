//! AI Semantic tools — 2 tools for embeddings and semantic cleaning
//!
//! Tools: semantic_cleaner, search_obsidian
//!
//! Tool functions are always registered. Feature-gated code inside
//! the function body controls the actual implementation.

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use tracing::instrument;

#[tool_router(router = tool_router_ai, vis = "pub")]
impl McpHandler {
    /// Semantic HTML cleaning with AI embeddings
    ///
    /// Fetches a URL, cleans its HTML content, and returns semantically
    /// chunked content with embeddings. Requires the `ai` feature.
    #[tool(
        description = "Fetch a URL, semantically clean its HTML content using AI embeddings, and return chunked content with vectors. Requires --features ai."
    )]
    #[instrument(skip(self), fields(url = %params.url))]
    async fn semantic_cleaner(
        &self,
        Parameters(params): Parameters<ScrapeUrlParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .ai
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        let _url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("invalid URL: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        #[cfg(feature = "ai")]
        {
            use std::sync::Arc;
            use webfang_ai::{ModelConfig, SemanticCleaner, SemanticCleanerImpl};

            // Fetch the page
            let client = self.state.container.http_client().as_ref();
            let response = client.get(_url.as_str()).await.map_err(|e| {
                McpError::internal_error(
                    format!("failed to fetch URL: {e}"),
                    Some(serde_json::Value::String(e.to_string())),
                )
            })?;
            let html = response.body;

            let config = ModelConfig::default();
            let cleaner = match SemanticCleanerImpl::new(config).await {
                Ok(c) => Arc::new(c),
                Err(e) => {
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "AI feature initialization failed: {e}"
                    ))]));
                },
            };

            match cleaner.clean(&html).await {
                Ok(chunks) => {
                    let json = serde_json::json!({
                        "chunks": chunks.iter().map(|c| serde_json::json!({
                            "id": c.id.to_string(),
                            "url": c.url,
                            "title": c.title,
                            "content": c.content,
                        })).collect::<Vec<_>>(),
                        "count": chunks.len(),
                    });
                    Ok(CallToolResult::success(vec![Content::text(
                        serde_json::to_string_pretty(&json).unwrap(),
                    )]))
                },
                Err(e) => Ok(CallToolResult::error(vec![Content::text(format!(
                    "semantic cleaning failed: {e}"
                ))])),
            }
        }

        #[cfg(not(feature = "ai"))]
        {
            Ok(CallToolResult::error(vec![Content::text(
                "AI feature not enabled. Rebuild with --features ai".to_string(),
            )]))
        }
    }

    /// Semantic search over Obsidian vault using embeddings
    #[tool(
        description = "Semantic search over Obsidian vault using ONNX Runtime embeddings. Returns top matching notes by cosine similarity. Requires --features ai."
    )]
    #[instrument(skip(self), fields(query = %params.query))]
    async fn search_obsidian(
        &self,
        Parameters(params): Parameters<SearchObsidianParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = self
            .state
            .semaphores
            .obsidian
            .acquire()
            .await
            .map_err(|e| McpError::internal_error(format!("semaphore error: {e}"), None))?;

        #[cfg(feature = "ai")]
        {
            let limit = params.limit.unwrap_or(10);

            // For now: search_obsidian requires vault path configuration
            // that is not yet wired into McpState. Return a meaningful
            // error directing users to configure their vault path.
            if params.vault_path.is_none() {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "search_obsidian requires --vault-path configuration. \
                         Set WEBFANG_OBSIDIAN_VAULT env var or configure vault in config.toml. \
                         Query: '{}', limit: {}",
                    params.query, limit
                ))]));
            }

            Ok(CallToolResult::error(vec![Content::text(format!(
                "search_obsidian semantic search not yet implemented. \
                 Vault path provided, but the embedding search pipeline is not wired. \
                 Query: '{}', limit: {}",
                params.query, limit
            ))]))
        }

        #[cfg(not(feature = "ai"))]
        {
            Ok(CallToolResult::error(vec![Content::text(
                "AI feature not enabled. Rebuild with --features ai for semantic search."
                    .to_string(),
            )]))
        }
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_ai()
}
