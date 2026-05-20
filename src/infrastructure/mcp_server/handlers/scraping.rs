//! Scraping Core tools — 8 tools for URL discovery and scraping
//!
//! Tools: scrape_url, scrape_with_config, scrape_batch, crawl_site,
//! crawl_with_sitemap, discover_urls, discover_sitemap, detect_spa

use rmcp::handler::server::tool::ToolRouter;
use super::McpHandler;
use super::super::state::McpState;

/// Build the partial tool router for scraping tools.
pub fn build_router(_state: &McpState) -> ToolRouter<McpHandler> {
    // TODO: Implement 8 scraping tools (PR 2)
    // Will use #[tool_router] macro on impl McpHandler block
    ToolRouter::new()
}
