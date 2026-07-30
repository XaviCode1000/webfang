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

/// Build an honest tool error (`isError:true`) carrying a Spanish message.
///
/// Shared by the AI handlers so feature-gated and operational failures are
/// reported as `CallToolResult::error` — never an MCP protocol error, never a
/// false success. Mirrors the slice-1 export handlers (see `export.rs`).
fn honest_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message.into())])
}

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
        let _permit = acquire_semaphore!(self, ai);

        let _url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("URL inválida: {e}"),
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
        let _permit = acquire_semaphore!(self, obsidian);

        // Deferred capability (follow-up issue #386): semantic vault search
        // needs an embed/rank surface beyond the `SemanticCleaner` trait.
        // Report an honest error — never a false success (REQ-04).
        Ok(honest_error(
            "funcionalidad no disponible: la búsqueda semántica en Obsidian aún no está implementada. Seguimiento en el issue #386.",
        ))
    }
}

pub fn build_router() -> ToolRouter<McpHandler> {
    McpHandler::tool_router_ai()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-02/04 contract: `honest_error` produces a `CallToolResult` that
    /// serializes with `isError:true` and carries the exact Spanish text —
    /// never a protocol error, never a false success. Mirrors the slice-1
    /// export-handler test (export.rs).
    #[test]
    fn honest_error_sets_is_error_and_spanish_text() {
        let result = honest_error("funcionalidad no disponible: prueba");

        // Serialize exactly as the MCP transport would, then assert the honest
        // error contract: isError:true plus the Spanish message.
        let json = serde_json::to_value(&result).expect("CallToolResult must serialize");
        assert_eq!(
            json.get("isError").and_then(|v| v.as_bool()),
            Some(true),
            "honest_error must set isError:true, got: {json}"
        );
        let text = json
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        assert!(
            text.contains("funcionalidad no disponible"),
            "honest Spanish error text expected, got: {text}"
        );
    }
}
