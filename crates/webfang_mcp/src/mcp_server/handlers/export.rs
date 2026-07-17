//! Export tools — 4 tools for output format conversion
//!
//! Tools: export_file, export_jsonl, export_vector,
//! process_export_pipeline

use super::McpHandler;
use rmcp::handler::server::tool::ToolRouter;

/// Build the partial tool router for export tools.
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::new()
}
