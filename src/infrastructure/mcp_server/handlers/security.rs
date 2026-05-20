//! Security & Diagnostics tools — 4 tools for WAF detection and metrics
//!
//! Tools: detect_waf, verify_waf_integrity, list_waf_providers,
//! get_scrape_metrics

use rmcp::handler::server::tool::ToolRouter;
use super::McpHandler;
use super::super::state::McpState;

/// Build the partial tool router for security tools.
pub fn build_router(_state: &McpState) -> ToolRouter<McpHandler> {
    // TODO: Implement 4 security tools (PR 2)
    ToolRouter::new()
}
