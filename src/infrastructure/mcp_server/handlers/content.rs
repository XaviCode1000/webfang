//! Content Processing tools — 7 tools for HTML/Markdown transformation
//!
//! Tools: extract_links, clean_html, convert_html_to_markdown,
//! highlight_code_blocks, convert_wiki_links, generate_frontmatter,
//! generate_rich_metadata

use rmcp::handler::server::tool::ToolRouter;
use super::McpHandler;
use super::super::state::McpState;

/// Build the partial tool router for content tools.
pub fn build_router(_state: &McpState) -> ToolRouter<McpHandler> {
    // TODO: Implement 7 content tools (PR 2)
    ToolRouter::new()
}
