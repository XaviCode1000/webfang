//! AI Semantic tools — 3 tools for embeddings and semantic cleaning
//!
//! Tools: semantic_clean, score_relevance, generate_embedding
//!
//! Feature-gated: only compiled with --features ai

use rmcp::handler::server::tool::ToolRouter;
use super::McpHandler;
use super::super::state::McpState;

/// Build the partial tool router for AI tools.
#[cfg(feature = "ai")]
pub fn build_router(_state: &McpState) -> ToolRouter<McpHandler> {
    // TODO: Implement 3 AI tools (PR 2)
    ToolRouter::new()
}

/// Stub for non-AI builds (returns empty router).
#[cfg(not(feature = "ai"))]
pub fn build_router(_state: &McpState) -> ToolRouter<McpHandler> {
    ToolRouter::new()
}
