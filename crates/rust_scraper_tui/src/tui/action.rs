//! Actions for the reactive TUI architecture.
//!
//! Actions represent events that have been processed by the event loop
//! and dispatched to components. They drive state changes in the UI.
//!
//! # Architecture
//!
//! Actions flow through the system as follows:
//! 1. Terminal events → crossterm → Event enum
//! 2. Tui event loop → Event enum → Action enum (via dispatcher)
//! 3. Components process Actions via `update()` → may produce new Actions
//! 4. App loop sends Actions back through the component chain

use serde::{Deserialize, Serialize};
use std::fmt;

use super::ScrapeProgress;

/// Application-level actions that drive UI state changes.
///
/// These are the "verbs" of the application — they represent
/// meaningful operations rather than raw input events.
///
/// Serde derives are provided for serialization (e.g., for
/// serializing actions in tests or debugging).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Internal timer tick for periodic updates
    Tick,
    /// Request to render the UI
    Render,
    /// Terminal was resized to (width, height)
    Resize(u16, u16),
    /// Suspend TUI (e.g., for background operations)
    Suspend,
    /// Resume TUI after suspend
    Resume,
    /// Exit the application
    Quit,
    /// Clear the terminal screen
    ClearScreen,
    /// An error occurred with a description
    Error(String),
    /// Toggle help overlay
    ToggleHelp,
    /// Close the currently open modal
    #[serde(skip)]
    CloseModal,
    /// URLs were confirmed by the user
    UrlConfirmed(Vec<String>),
    /// URL selection was cancelled
    UrlCancelled,
    /// Config form was submitted (carries optional JSON value)
    ConfigDone(Option<serde_json::Value>),
    /// Config form was cancelled
    ConfigCancelled,
    /// Progress update from the scraper
    #[serde(skip)]
    Progress(ScrapeProgress),
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tick => write!(f, "Tick"),
            Self::Render => write!(f, "Render"),
            Self::Resize(w, h) => write!(f, "Resize({w}, {h})"),
            Self::Suspend => write!(f, "Suspend"),
            Self::Resume => write!(f, "Resume"),
            Self::Quit => write!(f, "Quit"),
            Self::ClearScreen => write!(f, "ClearScreen"),
            Self::Error(e) => write!(f, "Error({e})"),
            Self::ToggleHelp => write!(f, "ToggleHelp"),
            Self::CloseModal => write!(f, "CloseModal"),
            Self::UrlConfirmed(urls) => write!(f, "UrlConfirmed({} urls)", urls.len()),
            Self::UrlCancelled => write!(f, "UrlCancelled"),
            Self::ConfigDone(_) => write!(f, "ConfigDone"),
            Self::ConfigCancelled => write!(f, "ConfigCancelled"),
            Self::Progress(p) => write!(f, "Progress({p:?})"),
        }
    }
}
