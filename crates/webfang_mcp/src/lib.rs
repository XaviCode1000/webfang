#![cfg_attr(not(test), deny(clippy::unwrap_used))]
//! WebFang MCP — Model Context Protocol server
//!
//! Exposes scraper tools to AI agents via MCP protocol.
//! Depends on `webfang_core` for domain types.

pub mod mcp_server;
