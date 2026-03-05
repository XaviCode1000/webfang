//! Markdown Processing Module (DEPRECATED)
//!
//! **This module is deprecated** - use `fast_html2md` or `legible` crate instead.
//! The new scraper implementation handles this automatically.
//!
//! This module is kept for backward compatibility and will be removed in v0.3.0.

use std::path::Path;

/// DEPRECATED: Use scraper::save_results instead
#[deprecated(
    since = "0.2.0",
    note = "Use scraper::save_results with OutputFormat::Markdown"
)]
pub fn process_and_save(
    _pages: &[super::scraper::ScrapedContent],
    _output_dir: &Path,
) -> Result<(), MarkdownError> {
    Err(MarkdownError::Deprecated)
}

/// Error type
#[derive(Debug)]
pub enum MarkdownError {
    #[deprecated(since = "0.2.0")]
    IoError(std::io::Error),
    #[deprecated(since = "0.2.0")]
    NoPagesProvided,
    /// New error type for deprecation
    Deprecated,
}

impl std::fmt::Display for MarkdownError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MarkdownError::Deprecated => {
                write!(
                    f,
                    "markdown module is deprecated - use scraper::save_results instead"
                )
            }
            _ => write!(f, "deprecated module"),
        }
    }
}

impl std::error::Error for MarkdownError {}
