//! Security & Diagnostics tools — 4 tools for WAF detection and metrics
//!
//! Tools: detect_waf, verify_waf_integrity, list_waf_providers,
//! get_scrape_metrics

use super::McpHandler;
use rmcp::handler::server::tool::ToolRouter;

/// Build the partial tool router for security tools.
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::new()
}
