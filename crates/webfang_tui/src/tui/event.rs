//! Terminal events for the reactive TUI architecture.
//!
//! Raw terminal events from crossterm, converted into our own
//! Event enum for type-safe event handling.
//!
//! # Architecture
//!
//! Events flow from the TUI event loop to components:
//! 1. crossterm captures raw terminal input
//! 2. Tui event loop converts to Event enum
//! 3. Components handle events via `handle_events()` or `handle_key_event()`
//!
//! Events are distinct from Actions:
//! - Events: raw terminal input (key presses, mouse, resize)
//! - Actions: application-level operations (Quit, Render, UrlConfirmed)

use crossterm::event::{KeyEvent, MouseEvent};

/// Raw terminal events from crossterm.
///
/// These are the lowest-level events in the system, representing
/// direct terminal input before any application-level processing.
#[derive(Debug, Clone)]
pub enum Event {
    /// TUI has been initialized
    Init,
    /// Request to quit the application
    Quit,
    /// An error occurred in the event stream
    Error,
    /// Event stream was closed
    Closed,
    /// Periodic timer tick
    Tick,
    /// Request to render the UI
    Render,
    /// Terminal focus was gained
    FocusGained,
    /// Terminal focus was lost
    FocusLost,
    /// Bracketed paste content received
    Paste(String),
    /// A keyboard key was pressed
    Key(KeyEvent),
    /// A mouse event occurred
    Mouse(MouseEvent),
    /// Terminal was resized to (width, height)
    Resize(u16, u16),
}
