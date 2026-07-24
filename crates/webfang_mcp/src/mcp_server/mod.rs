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
pub mod handlers;
pub mod params;
pub mod selector_service;
pub mod server;
pub mod state;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{CallToolResult, ListToolsResult, ServerCapabilities, ServerInfo, Tool};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer};

pub use state::McpState;

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
