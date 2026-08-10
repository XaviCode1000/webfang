//! MCP Server — Model Context Protocol bridge for AI agents
//!
//! Exposes 35 scraper tools across 8 categories via Streamable HTTP.
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

use webfang_core::di::ContainerExt;

pub use state::McpState;

/// Build a Container with optional AI semantic cleaner wiring.
///
/// This is used by both `mcp_server_http.rs` and `mcp_server_stdio.rs`
/// binaries to avoid code duplication. Construction is OPT-IN via the
/// `enable_ai` flag — a plain smoke run does not download the ONNX model.
///
/// # Panics
///
/// Panics if the container cannot be created from the default config.
/// This should never happen as the default config always produces a valid container.
#[cfg(feature = "ai")]
pub async fn build_container_with_ai(enable_ai: bool) -> webfang_core::di::Container {
    use webfang_ai::AiModel;
    use webfang_ai::ModelConfig;
    use webfang_ai::SemanticCleanerImpl;

    let config = webfang_core::config::Config::default();
    let container = webfang_core::di::Container::from_config(config)
        .await
        .ok()
        .unwrap_or_else(|| panic!("failed to create container: default config should be valid"));

    if !enable_ai {
        return container;
    }

    let variant = AiModel::from_env_or_default();
    let model_config = ModelConfig::default().with_model_variant(variant);

    match SemanticCleanerImpl::new(model_config).await {
        Ok(cleaner) => {
            let (pool, tokenizer) = cleaner.shared_inference();
            let container = container.with_cleaner(std::sync::Arc::new(cleaner));
            ai_wiring::wire_ai_ports(container, pool, tokenizer).await
        },
        Err(e) => {
            tracing::warn!("semantic cleaner unavailable, continuing without AI: {e}");
            container
        },
    }
}

/// Build a basic Container without AI wiring (for non-AI builds).
#[cfg(not(feature = "ai"))]
pub async fn build_container_with_ai(_enable_ai: bool) -> webfang_core::di::Container {
    let config = webfang_core::config::Config::default();
    webfang_core::di::Container::from_config(config)
        .await
        .expect("failed to create container")
}

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
