#![cfg_attr(not(test), deny(clippy::unwrap_used))]
#![cfg_attr(not(test), deny(clippy::expect_used))]
#![deny(missing_docs)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
//! WebFang TUI — Terminal User Interface
//!
//! Provides the interactive TUI for configuration and URL selection.
//! Depends on `webfang_core` for domain types.

pub mod tui;
