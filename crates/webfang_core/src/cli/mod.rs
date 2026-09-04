//! CLI module — argument parsing, error handling, completions, config.
//!
//! Clean Architecture Adapters layer: all CLI-related utilities.

/// CLI argument definitions using clap derive macros.
pub mod args;
pub mod commands;
pub mod completions;
pub mod config;
pub mod crash_points;
pub mod elastic;
pub mod error;
pub mod export_flow;
pub mod orchestrator;
pub mod parse;
pub mod preflight;
pub mod scrape_flow;
pub mod shutdown;
pub(crate) mod spec_command;
pub mod summary;
pub mod url_discovery;

pub use crate::CliExit;
pub use args::{Args, Commands, Shell};
