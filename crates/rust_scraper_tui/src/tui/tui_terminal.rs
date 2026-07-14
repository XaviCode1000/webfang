//! Tui struct — terminal management only.
//!
//! Wraps a ratatui Terminal with setup, teardown, drawing, and sizing.
//! Does NOT manage the event loop — that's handled by `App::run()`.
//!
//! # Architecture
//!
//! Tui is the I/O layer for terminal operations:
//! - Terminal setup/teardown (alternate screen, raw mode, mouse capture)
//! - Panic hook to restore terminal on crash
//! - Drawing and sizing helpers
//!
//! The event loop lives in App, which owns the crossterm EventStream
//! and manages tick/render intervals directly via `tokio::select!`.
//!
//! # Usage
//!
//! ```no_run
//! # use anyhow::Result;
//! # async fn example() -> Result<()> {
//! let mut tui = rust_scraper::adapters::tui::Tui::new()?;
//! tui.enter()?;
//! // ... event loop managed by App ...
//! tui.exit()?;
//! # Ok(())
//! # }
//! ```

use std::io::{stdout, Stdout};
use std::ops::{Deref, DerefMut};

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;

use crossterm::execute;

/// Convenience alias for ratatui Frame.
pub type Frame<'a> = ratatui::Frame<'a>;

/// Terminal manager — setup, teardown, draw, resize.
///
/// Does NOT manage the event loop. Use [`super::app::App::run()`] for the
/// unified event loop that combines crossterm events, tick
/// intervals, and action processing.
pub struct Tui {
    /// The underlying ratatui Terminal
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    /// Create a new Tui instance.
    ///
    /// Does NOT enter the terminal — call [`enter()`](Self::enter) to do that.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal creation fails.
    pub fn new() -> Result<Self> {
        let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        Ok(Self { terminal })
    }

    /// Enter the terminal: enable raw mode, alternate screen, and mouse capture.
    ///
    /// Also sets up a panic hook so the terminal is restored on crash.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal setup fails.
    pub fn enter(&mut self) -> Result<()> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        self.terminal.clear()?;
        self.terminal.hide_cursor()?;
        setup_panic_hook();
        Ok(())
    }

    /// Exit the terminal: restore raw mode, leave alternate screen.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal restoration fails.
    pub fn exit(&mut self) -> Result<()> {
        disable_raw_mode()?;
        execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Draw to the terminal using the provided drawing function.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal draw fails.
    pub fn draw(&mut self, draw_fn: impl FnOnce(&mut Frame)) -> Result<()> {
        self.terminal.draw(draw_fn)?;
        Ok(())
    }

    /// Get the current terminal size.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal size query fails.
    pub fn size(&self) -> Result<Size> {
        Ok(self.terminal.size()?)
    }

    /// Suspend the TUI (exit terminal, keep state).
    ///
    /// Useful for running external commands during the session.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal restoration fails.
    pub fn suspend(&mut self) -> Result<()> {
        self.exit()?;
        Ok(())
    }

    /// Resume the TUI (re-enter terminal).
    ///
    /// # Errors
    ///
    /// Returns an error if terminal setup fails.
    pub fn resume(&mut self) -> Result<()> {
        self.enter()?;
        Ok(())
    }
}

impl Deref for Tui {
    type Target = Terminal<CrosstermBackend<Stdout>>;

    fn deref(&self) -> &Self::Target {
        &self.terminal
    }
}

impl DerefMut for Tui {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.terminal
    }
}

/// Set up a panic hook that restores the terminal on panic.
///
/// Each restoration step runs independently so that a partial
/// failure (e.g., broken stdout) doesn't prevent other steps.
fn setup_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Each step runs independently — failure in one doesn't block others
        // Following **err-result-over-panic**: ignore errors in cleanup path
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
        let _ = execute!(std::io::stdout(), crossterm::cursor::Show);
        eprintln!("Application panicked. Terminal restored.");
        original_hook(panic_info);
    }));
}

#[cfg(test)]
mod tests {
    use std::io::{stderr, stdin, stdout, IsTerminal};

    use super::*;

    /// Returns true only when terminal setup can run against real TTY streams.
    /// Some local command runners are not CI, but still do not expose a TTY.
    fn has_terminal() -> bool {
        std::env::var("CI").is_err()
            && stdin().is_terminal()
            && stdout().is_terminal()
            && stderr().is_terminal()
    }

    #[test]
    fn test_tui_new() {
        if !has_terminal() {
            return;
        }
        let tui = Tui::new();
        assert!(tui.is_ok());
    }

    #[test]
    fn test_tui_suspend_resume() {
        if !has_terminal() {
            return;
        }
        let mut tui = Tui::new().unwrap();
        assert!(tui.enter().is_ok());
        assert!(tui.suspend().is_ok());
        assert!(tui.resume().is_ok());
        assert!(tui.exit().is_ok());
    }
}
