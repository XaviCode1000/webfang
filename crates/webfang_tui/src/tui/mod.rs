//! TUI Adapter Module
//!
//! Interactive terminal UI for URL selection.
//! This is a Delivery Mechanism (Adapter layer).
//!
//! # Architecture
//!
//! The TUI is an adapter that:
//! 1. Receives discovered URLs from Application layer
//! 2. Renders interactive UI for user selection
//! 3. Returns selected URLs back to orchestrator
//!
//! # Examples
//!
//! ```no_run
//! use webfang_tui::tui;
//! use url::Url;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let urls = vec![
//!     Url::parse("https://example.com/1")?,
//!     Url::parse("https://example.com/2")?,
//! ];
//! let selected = tui::run_selector(&urls).await?;
//! # Ok(())
//! # }
//! ```

pub mod action;
pub mod app;
pub mod component;
pub mod event;
pub mod theme;
pub mod tui_terminal;

pub mod collapsible_config;
mod config_form;
mod config_forms;
mod error_log_widget;
pub mod modal;
pub mod progress_types {
    //! Path-compat shim — canonical home is now `webfang_core::domain::entities::progress`.
    pub use webfang_core::domain::entities::progress::*;
}
mod progress_widget;
mod url_selector;

pub use action::Action;
pub use app::{App, AppResult};
pub use component::{AppMode, Component, Header, StatusBar};
pub use error_log_widget::{ErrorLogWidget, DEFAULT_MAX_ERRORS};
pub use event::Event;
pub use tui_terminal::Tui;

pub use collapsible_config::CollapsibleConfig;
pub use config_form::ConfigFormState;
pub use progress_types::{
    ErrorEntry, ErrorType, ProgressState, ScrapeError, ScrapeProgress, ScrapeStatus, UrlState,
};
pub use progress_widget::{ProgressIcons, ProgressWidget};
pub use url_selector::{run_selector, UrlSelector, UrlSelectorState};

use thiserror::Error;

/// TUI adapter errors
///
/// Follows err-thiserror-lib rule for library error types.
#[derive(Debug, Error)]
pub enum TuiError {
    #[error("Terminal setup failed: {0}")]
    /// Terminal backend initialization failed
    TerminalSetup(#[from] std::io::Error),

    #[error("User interrupted")]
    /// The user interrupted the TUI session
    Interrupted,
}

/// Result type for TUI operations
pub type Result<T> = std::result::Result<T, TuiError>;
