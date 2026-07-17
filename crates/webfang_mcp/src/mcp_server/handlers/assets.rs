//! Asset Management tools — 1 tool for downloading images/documents
//!
//! Tools: download_assets

use super::McpHandler;
use rmcp::handler::server::tool::ToolRouter;

/// Build the partial tool router for asset tools.
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::new()
}
