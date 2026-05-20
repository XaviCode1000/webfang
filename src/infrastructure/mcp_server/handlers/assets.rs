//! Asset Management tools — 1 tool for downloading images/documents
//!
//! Tools: download_assets

use rmcp::handler::server::tool::ToolRouter;
use super::McpHandler;
use super::super::state::McpState;

/// Build the partial tool router for asset tools.
pub fn build_router(_state: &McpState) -> ToolRouter<McpHandler> {
    // TODO: Implement 1 asset tool (PR 2)
    ToolRouter::new()
}
