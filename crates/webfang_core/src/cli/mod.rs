//! CLI module — argument parsing, error handling, completions, config.
//!
//! Clean Architecture Adapters layer: all CLI-related utilities.

#[allow(missing_docs)]
pub mod args;
pub mod commands;
pub mod completions;
pub mod config;
pub mod error;
pub mod export_flow;
pub mod orchestrator;
pub mod preflight;
pub mod scrape_flow;
pub mod summary;
pub mod url_discovery;
pub mod wizard;

pub use crate::CliExit;
pub use args::{Args, Commands, Shell};

/// Result of URL selection.
#[derive(Debug)]
pub(crate) enum SelectedUrls {
    Urls(Vec<url::Url>),
    None, // User cancelled or no selection
    Error(CliExit),
}
