//! Export tools — 4 tools for output format conversion
//!
//! Tools: export_file, export_jsonl, export_vector,
//! process_export_pipeline

use rmcp::handler::server::tool::ToolRouter;
use super::McpHandler;
use super::super::state::McpState;

/// Build the partial tool router for export tools.
pub fn build_router(_state: &McpState) -> ToolRouter<McpHandler> {
    // TODO: Implement 4 export tools (PR 2)
    ToolRouter::new()
}
