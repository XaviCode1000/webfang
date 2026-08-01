//! AI Semantic tools — 2 tools for embeddings and semantic cleaning
//!
//! Tools: semantic_cleaner, search_obsidian
//!
//! Tool functions are always registered. `semantic_cleaner` runs when a
//! semantic cleaner is injected into the `Container` (constructed behind the
//! `ai` feature); without one it returns an honest feature-gated error.
//! `search_obsidian` requires `embedding_port`, `note_repository`, and
//! `text_chunker` to be injected (#386); without them it returns an honest
//! feature-gated error.

use super::McpHandler;
use crate::mcp_server::params::*;
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, model::Content, ErrorData as McpError};
use tracing::instrument;
use webfang_core::application::vault_search::{VaultSearchResult, VaultSearchService};
use webfang_core::domain::DocumentChunk;

/// Build an honest tool error (`isError:true`) carrying a Spanish message.
///
/// Shared by the AI handlers so feature-gated and operational failures are
/// reported as `CallToolResult::error` — never an MCP protocol error, never a
/// false success. Mirrors the slice-1 export handlers (see `export.rs`).
fn honest_error(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![Content::text(message.into())])
}

/// Success envelope for `semantic_cleaner` (REQ-01).
///
/// Embeddings are inline and full — each chunk carries its vector (truncating
/// would violate REQ-01). The summary fields give a quick overview without
/// scanning `documents`.
#[derive(serde::Serialize)]
struct SemanticCleanResponse {
    /// The URL that was fetched and cleaned.
    url: String,
    /// Number of semantic chunks produced.
    chunks: usize,
    /// Embedding dimensionality (0 when the cleaner produced no embeddings).
    embedding_dim: usize,
    /// The cleaned chunks, each with its embedding populated.
    documents: Vec<DocumentChunk>,
}

/// Success envelope for `search_obsidian` (#386).
#[derive(serde::Serialize)]
struct VaultSearchResponse {
    /// The search query.
    query: String,
    /// Number of results returned.
    results: usize,
    /// The matching note chunks, sorted by descending relevance.
    documents: Vec<VaultSearchResult>,
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

        // REQ-03: reject malformed URLs first (invalid-params), regardless of
        // cleaner presence — no fetch, no cleaning.
        let _url = url::Url::parse(&params.url).map_err(|e| {
            McpError::invalid_params(
                format!("URL inválida: {e}"),
                Some(serde_json::Value::String("url".to_string())),
            )
        })?;

        // REQ-02: absent cleaner (ai feature off) -> honest Spanish error.
        let Some(cleaner) = self.state.container.cleaner() else {
            return Ok(honest_error(
                "funcionalidad no disponible: limpieza semántica con IA. Reconstruye con --features ai para habilitarla.",
            ));
        };

        // REQ-01: fetch the page via the existing HTTP port, then clean it.
        // Operational failures map to honest errors — never a false success.
        let response = match self.state.container.http_client().get(&params.url).await {
            Ok(resp) => resp,
            Err(e) => return Ok(honest_error(format!("no se pudo obtener la página: {e}"))),
        };
        let documents = match cleaner.clean(&response.body).await {
            Ok(docs) => docs,
            Err(e) => return Ok(honest_error(format!("error al limpiar el contenido: {e}"))),
        };

        let chunks = documents.len();
        let embedding_dim = documents
            .first()
            .and_then(|doc| doc.embeddings.as_ref())
            .map(Vec::len)
            .unwrap_or(0);
        let envelope = SemanticCleanResponse {
            url: params.url.clone(),
            chunks,
            embedding_dim,
            documents,
        };
        let payload = match serde_json::to_string(&envelope) {
            Ok(json) => json,
            Err(e) => {
                return Ok(honest_error(format!(
                    "error al serializar la respuesta: {e}"
                )))
            },
        };
        Ok(CallToolResult::success(vec![Content::text(payload)]))
    }

    /// Semantic search over Obsidian vault using embeddings
    ///
    /// Searches indexed vault notes by embedding the query and ranking
    /// against pre-computed chunk embeddings via cosine similarity.
    /// Requires `embedding_port`, `note_repository`, and `text_chunker`
    /// to be injected into the Container (#386).
    #[tool(
        description = "Semantic search over Obsidian vault using ONNX Runtime embeddings. Returns top matching notes by cosine similarity. Requires --features ai."
    )]
    #[instrument(skip(self), fields(query = %params.query))]
    async fn search_obsidian(
        &self,
        Parameters(params): Parameters<SearchObsidianParams>,
    ) -> Result<CallToolResult, McpError> {
        let _permit = acquire_semaphore!(self, obsidian);

        // Check all three required ports are available.
        let Some(embedding) = self.state.container.embedding_port() else {
            return Ok(honest_error(
                "funcionalidad no disponible: búsqueda semántica en Obsidian. Reconstruye con --features ai para habilitarla.",
            ));
        };
        let Some(repo) = self.state.container.note_repository() else {
            return Ok(honest_error(
                "funcionalidad no disponible: no hay repositorio de notas configurado. Verifica la configuración de persistencia.",
            ));
        };
        let Some(chunker) = self.state.container.text_chunker() else {
            return Ok(honest_error(
                "funcionalidad no disponible: no hay chunker de texto configurado. Reconstruye con --features ai.",
            ));
        };

        let service = VaultSearchService::new(embedding, repo, chunker);
        let limit = params.limit.unwrap_or(10);

        match service.search(&params.query, limit).await {
            Ok(results) => {
                let envelope = VaultSearchResponse {
                    query: params.query.clone(),
                    results: results.len(),
                    documents: results,
                };
                match serde_json::to_string(&envelope) {
                    Ok(json) => Ok(CallToolResult::success(vec![Content::text(json)])),
                    Err(e) => Ok(honest_error(format!(
                        "error al serializar la respuesta: {e}"
                    ))),
                }
            }
            Err(e) => Ok(honest_error(format!(
                "error en la búsqueda semántica: {e}"
            ))),
        }
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
