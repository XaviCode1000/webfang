#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
// Raw process-env mutations must go through webfang_test_utils (#1126,
// #1349). Ungated on purpose: cfg(test) mods in this crate are covered too.
#![deny(clippy::disallowed_methods)]
#![deny(missing_docs)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
// Explicit per docs-truthfulness audit (S5): these rustdoc lints are
// allow-by-default, so CI's `-D warnings` never reaches them. Declare intent.
#![allow(rustdoc::missing_crate_level_docs)]
#![allow(rustdoc::private_doc_tests)]
#![allow(rustdoc::unescaped_backticks)]
//! WebFang MCP — Model Context Protocol server
//!
//! Exposes scraper tools to AI agents via MCP protocol.
//! Depends on `webfang_core` for domain types.

pub mod mcp_server;
