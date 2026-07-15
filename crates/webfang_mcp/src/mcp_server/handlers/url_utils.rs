//! URL Utility tools — 6 tools for URL manipulation
//!
//! Tools: validate_url, extract_domain, normalize_url,
//! match_url_pattern, is_internal_link, url_to_file_path

use super::McpHandler;
use rmcp::handler::server::tool::ToolRouter;

/// Build the partial tool router for URL tools.
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::new()
}
