//! Content processing adapters — Infrastructure layer
//!
//! Each adapter implements [`ContentProcessor`](crate::domain::content_processor::ContentProcessor)
//! with a distinct cleaning strategy:
//!
//! - [`SemanticProcessor`] — Readability extraction + naive tag strip (pipeline scraper)
//! - [`AggressiveProcessor`] — lol_html boilerplate removal + block-level strip (download pipeline)
//! - [`McpProcessor`] — lol_html element handlers, preserves semantic HTML (MCP tools)

pub mod aggressive;
pub mod mcp;
pub mod semantic;

pub use aggressive::AggressiveProcessor;
pub use mcp::McpProcessor;
pub use semantic::SemanticProcessor;
