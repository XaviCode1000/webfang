//! MCP Handler modules — tool implementations organized by category
//!
//! Each module provides a `#[tool_router]` impl block with tools and a
//! `build_router()` function that returns a partial `ToolRouter<McpHandler>`.
//! All routers are combined with the `+` operator in `build_tool_router()`.

pub use super::McpHandler;

use rmcp::handler::server::tool::ToolRouter;

pub mod ai;
pub mod assets;
pub mod axtree;
pub mod content;
pub mod export;
pub mod obsidian;
pub mod scraping;
pub mod security;
pub mod url_utils;

/// Build the combined ToolRouter from all 9 category modules.
///
/// After combining the category routers, the schema bridge overrides the
/// advertised input schemas of tools whose parameters overlap an
/// OptionsSpec entry (ADR-002 slice 4, #940).
pub fn build_tool_router() -> ToolRouter<McpHandler> {
    let mut router = scraping::build_router()
        + content::build_router()
        + export::build_router()
        + url_utils::build_router()
        + security::build_router()
        + obsidian::build_router()
        + assets::build_router()
        + ai::build_router()
        + axtree::build_router();
    crate::mcp_server::schema_bridge::apply_overrides(&mut router);
    router
}
