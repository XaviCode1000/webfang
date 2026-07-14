//! MCP Handler modules — tool implementations organized by category
//!
//! Each module provides a `build_router()` function that returns a partial
//! `ToolRouter<McpHandler>`. All routers are combined with the `+` operator.
//!
//! Note: All 37 tools are defined in the parent mod.rs #[tool_router] block.
//! These submodules exist for future modularization but currently return
//! empty routers.

use super::McpHandler;
use rmcp::handler::server::tool::ToolRouter;

pub mod ai;
pub mod assets;
pub mod content;
pub mod export;
pub mod obsidian;
pub mod scraping;
pub mod security;
pub mod url_utils;

/// Build the combined ToolRouter from all 8 category modules.
pub fn build_tool_router() -> ToolRouter<McpHandler> {
    scraping::build_router()
        + content::build_router()
        + export::build_router()
        + url_utils::build_router()
        + security::build_router()
        + obsidian::build_router()
        + assets::build_router()
        + ai::build_router()
}
