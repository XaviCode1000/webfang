//! MCP Server — Model Context Protocol bridge for AI agents
//!
//! Exposes 37 scraper tools across 8 categories via Streamable HTTP.
//! Architecture:
//! - `state.rs` — McpState with embedded Container + per-category semaphores
//! - `server.rs` — Axum router + StreamableHttpService setup
//! - `handlers/` — 8 handler modules (one per tool category)
//!
//! Backpressure: Each category has its own tokio::sync::Semaphore
//! to prevent resource exhaustion on constrained hardware.

pub mod state;
pub mod server;
pub mod handlers;

use rmcp::handler::server::tool::ToolRouter;
use rmcp::tool_handler;
use rmcp::tool_router;
use rmcp::tool;
use rmcp::handler::server::ServerHandler;
use rmcp::{ErrorData as McpError, model::{CallToolResult, Content}};
pub use state::McpState;

/// Main MCP handler struct.
///
/// Holds the application state and combined tool router.
/// All 37 tools are registered via `#[tool_router]` macros
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
            state: state.clone(),
            tool_router: Self::tool_router() + handlers::build_tool_router(&state),
        }
    }
}

/// Primary tool router — generates the default `tool_router()` method
/// that `#[tool_handler]` requires. Additional tools from category
/// modules are combined in `handlers::build_tool_router()`.
#[tool_router]
impl McpHandler {
    /// Stub tool to verify MCP server is operational.
    /// Will be replaced by real tools in PR 2.
    #[tool(description = "Ping the MCP server to verify it is operational")]
    async fn mcp_ping(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text("pong")]))
    }
}

/// Implement ServerHandler for McpHandler.
///
/// The #[tool_handler] macro generates call_tool and list_tools
/// methods that delegate to self.tool_router.
#[tool_handler]
impl ServerHandler for McpHandler {}
