#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
//! WebFang TUI — Terminal User Interface
//!
//! Provides the interactive TUI for configuration and URL selection.
//! Depends on `webfang_core` for domain types.

pub mod tui;
