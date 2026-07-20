//! AI Semantic tools — 2 tools for embeddings and semantic cleaning
//!
//! Tools: semantic_cleaner, search_obsidian
//!
//! Tool functions are always registered. The ai feature is not available
//! in webfang_mcp — these always return a "not implemented" error.

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

        Ok(CallToolResult::error(vec![Content::text(
            "AI feature not available in webfang_mcp. Rebuild webfang_cli with --features ai instead.".to_string(),
        )]))
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

        Ok(CallToolResult::error(vec![Content::text(
            "AI feature not available in webfang_mcp. Rebuild webfang_cli with --features ai instead.".to_string(),
        )]))
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_ai()
}
