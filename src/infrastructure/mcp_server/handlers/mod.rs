//! MCP Handler modules — tool implementations organized by category
//!
//! Each module provides a `build_router()` function that returns a partial
//! `ToolRouter<McpHandler>`. All routers are combined with the `+` operator.

use rmcp::handler::server::tool::ToolRouter;
use super::McpHandler;
use super::state::McpState;

pub mod scraping;
pub mod content;
pub mod export;
pub mod url_utils;
pub mod security;
pub mod obsidian;
pub mod ai;
pub mod assets;

/// Build the combined ToolRouter from all 8 category modules.
pub fn build_tool_router(state: &McpState) -> ToolRouter<McpHandler> {
    scraping::build_router(state)
        + content::build_router(state)
        + export::build_router(state)
        + url_utils::build_router(state)
        + security::build_router(state)
        + obsidian::build_router(state)
        + assets::build_router(state)
}
