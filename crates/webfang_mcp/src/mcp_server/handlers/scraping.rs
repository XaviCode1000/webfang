//! Scraping Core tools — stub module (tools implemented in mod.rs)
//!
//! Tools: scrape_url, scrape_with_options, scrape_batch, crawl_site,
//! crawl_with_sitemap, discover_urls, discover_sitemap, detect_spa

use super::McpHandler;
use rmcp::handler::server::tool::ToolRouter;

/// Build the partial tool router for scraping tools.
/// Tools are defined in the parent mod.rs #[tool_router] block.
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::new()
}
