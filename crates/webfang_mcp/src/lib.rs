#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
// Raw process-env mutations must go through webfang_test_utils (#1126,
// #1349). Ungated on purpose: cfg(test) mods in this crate are covered too.
#![deny(clippy::disallowed_methods)]
#![deny(missing_docs)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
//! WebFang MCP — Model Context Protocol server
//!
//! Exposes scraper tools to AI agents via MCP protocol.
//! Depends on `webfang_core` for domain types.

pub mod mcp_server;
