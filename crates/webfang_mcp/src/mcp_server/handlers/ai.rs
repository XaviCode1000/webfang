//! AI Semantic tools — 3 tools for embeddings and semantic cleaning
//!
//! Tools: semantic_clean, score_relevance, generate_embedding
//!
//! Feature-gated: only compiled with --features ai

use super::McpHandler;
use rmcp::handler::server::tool::ToolRouter;

/// Build the partial tool router for AI tools.
#[cfg(feature = "ai")]
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::new()
}

/// Stub for non-AI builds (returns empty router).
#[cfg(not(feature = "ai"))]
pub fn build_router() -> ToolRouter<McpHandler> {
    ToolRouter::new()
}
