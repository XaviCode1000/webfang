//! MCP Server — Model Context Protocol bridge for AI agents
//!
//! Exposes 36 scraper tools across 9 categories via Streamable HTTP.
//! Architecture:
//! - `state.rs` — McpState with embedded Container + per-category semaphores
//! - `server.rs` — Axum router + StreamableHttpService setup
//! - `handlers/` — 8 handler modules (one per tool category)
//!
//! Backpressure: Each category has its own tokio::sync::Semaphore
//! to prevent resource exhaustion on constrained hardware.

#[macro_use]
pub mod macros;
pub mod auth;
pub mod handlers;
pub mod metrics;
pub mod panic_hook;
pub mod params;
pub mod selector_service;
pub mod server;
pub mod ssrf;
pub mod state;
pub mod validation;

/// Vault-search AI port wiring (#433) — only compiled with the `ai` feature.
#[cfg(feature = "ai")]
pub mod ai_wiring;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{CallToolResult, ListToolsResult, ServerCapabilities, ServerInfo, Tool};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer};

use std::sync::Arc;
use webfang_core::di::ContainerExt;

pub use state::McpState;

/// Build a Container — fast container construction, no model resolution
/// happens here (#759).
///
/// This is used by both `mcp_server_http.rs` and `mcp_server_stdio.rs`
/// binaries to avoid code duplication. The AI wiring that used to run inside
/// this function (and block the MCP `initialize` handshake behind the hf_hub
/// model resolution) is now performed lazily by [`spawn_ai_wiring`] after the
/// server starts serving (#759).
///
/// # Panics
///
/// Panics if the container cannot be created from the default config.
/// This should never happen as the default config always produces a valid container.
pub async fn build_container() -> webfang_core::di::Container {
    let config = webfang_core::config::Config::default();
    webfang_core::di::Container::from_config(config)
        .await
        .ok()
        .unwrap_or_else(|| panic!("failed to create container: default config should be valid"))
}

/// Kick off the lazy AI port wiring in a background task (#759).
///
/// Shares the same `Arc<Container>` that the MCP server already holds and
/// injects the AI ports (semantic cleaner, embedding, chunker, notes) after
/// the server has started serving. This unblocks the MCP `initialize`
/// handshake, which previously waited on the hf_hub model resolution
/// (~390 MB download on a cold cache). During warmup the AI tools degrade to
/// their pre-existing honest "not available" error.
#[cfg(feature = "ai")]
pub fn spawn_ai_wiring(container: Arc<webfang_core::application::container::Container>) {
    use tracing::Instrument;

    let variant = webfang_ai::AiModel::from_env_or_default();
    let span = tracing::info_span!("ai_lazy_wiring", model = variant.display_name());

    tokio::spawn(
        async move {
            let model_config = webfang_ai::ModelConfig::default().with_model_variant(variant);
            match webfang_ai::SemanticCleanerImpl::new(model_config).await {
                Ok(cleaner) => {
                    let (pool, tokenizer) = cleaner.shared_inference();
                    let cleaner: Arc<dyn webfang_core::domain::semantic_cleaner::SemanticCleaner> =
                        Arc::new(cleaner);
                    container.inject_vault_ports(
                        webfang_core::application::container::VaultAiPorts {
                            cleaner: Some(cleaner),
                            ..Default::default()
                        },
                    );
                    ai_wiring::wire_ai_ports(&container, pool, tokenizer).await;
                    tracing::info!("AI ports wired (lazy, post-handshake)");
                },
                Err(e) => tracing::warn!(error = %e, "AI warmup failed; continuing without AI"),
            }
        }
        .instrument(span),
    );
}

/// No-op placeholder when the `ai` feature is not compiled in (#759).
#[cfg(not(feature = "ai"))]
pub fn spawn_ai_wiring(_container: Arc<webfang_core::application::container::Container>) {}

/// Main MCP handler struct.
///
/// Holds the application state and combined tool router.
/// All 35 tools are registered via `#[tool_router]` macros
/// in the handler submodules.
#[derive(Clone)]
pub struct McpHandler {
    /// Shared application state (DI container + semaphores)
    pub state: McpState,
    /// Combined tool router from all 8 categories
    pub tool_router: ToolRouter<Self>,
}

impl McpHandler {
    /// Create a new MCP handler with the given state.
    pub fn new(state: McpState) -> Self {
        Self {
            state,
            tool_router: handlers::build_tool_router(),
        }
    }
}

/// Implement ServerHandler for McpHandler.
///
/// Uses the combined `self.tool_router` field (all 8 category routers)
/// for tool dispatch, listing, and lookup.
impl ServerHandler for McpHandler {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            meta: None,
            next_cursor: None,
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tool_router.get(name).cloned()
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::from_build_env())
    }
}
