//! Tui struct — terminal management and async event loop.
//!
//! Based on the ratatui Component Architecture pattern:
//! <https://github.com/ratatui/templates/tree/main/component>
//!
//! # Architecture
//!
//! The Tui struct wraps a ratatui Terminal and manages:
//! - Terminal setup/teardown (alternate screen, raw mode)
//! - Async event loop (crossterm events, tick, render)
//! - Event channel (unbounded MPSC) for dispatching to components
//! - Graceful cancellation via AtomicBool
//!
//! Cancellation uses `Arc<AtomicBool>` instead of `tokio_util::CancellationToken`
//! because `tokio-util` is not a project dependency.

use std::io::{stdout, Stdout};
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, EventStream, KeyEventKind};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use futures::{FutureExt, StreamExt};
use ratatui::prelude::*;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::Event;

/// Convenience alias for ratatui Frame.
pub type Frame<'a> = ratatui::Frame<'a>;

/// Terminal manager and async event loop.
///
/// Usage:
/// ```no_run
/// # use anyhow::Result;
/// # async fn example() -> Result<()> {
/// let mut tui = Tui::new()?;
/// tui.enter()?;
/// // ... event loop ...
/// tui.exit()?;
/// # Ok(())
/// # }
/// ```
pub struct Tui {
    /// The underlying ratatui Terminal
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    /// Sender for dispatching terminal events
    pub event_tx: UnboundedSender<Event>,
    /// Receiver for consuming terminal events
    event_rx: UnboundedReceiver<Event>,
    /// Cancellation flag — set to true to stop the event loop
    cancelled: Arc<AtomicBool>,
    /// Tick rate in Hz (default: 4.0 = 250ms)
    tick_rate: f64,
    /// Frame rate in Hz (default: 60.0 = ~16ms)
    frame_rate: f64,
}

impl Tui {
    /// Create a new Tui instance.
    ///
    /// Does NOT enter the terminal — call `enter()` to do that.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal creation fails.
    pub fn new() -> Result<Self> {
        let terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Ok(Self {
            terminal,
            event_tx,
            event_rx,
            cancelled: Arc::new(AtomicBool::new(false)),
            tick_rate: 4.0,
            frame_rate: 60.0,
        })
    }

    /// Set the tick rate in Hz (default: 4.0).
    #[must_use]
    pub fn tick_rate(mut self, rate: f64) -> Self {
        self.tick_rate = rate;
        self
    }

    /// Set the frame rate in Hz (default: 60.0).
    #[must_use]
    pub fn frame_rate(mut self, rate: f64) -> Self {
        self.frame_rate = rate;
        self
    }

    /// Enter the terminal: enable raw mode, alternate screen, and start the event loop.
    ///
    /// Spawns a tokio task that listens for:
    /// - crossterm keyboard/mouse/resize events
    /// - Periodic tick events
    /// - Periodic render events
    ///
    /// # Errors
    ///
    /// Returns an error if terminal setup fails.
    pub fn enter(&mut self) -> Result<()> {
        self.cancelled.store(false, Ordering::Relaxed);

        crossterm::terminal::enable_raw_mode()?;
        crossterm::execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
        self.terminal.clear()?;
        self.terminal.hide_cursor()?;

        let event_tx = self.event_tx.clone();
        let cancelled = self.cancelled.clone();
        let tick_rate = self.tick_rate;
        let frame_rate = self.frame_rate;

        // Spawn async event loop
        tokio::spawn(async move {
            let mut event_stream = EventStream::new();
            let mut tick_interval = tokio::time::interval(Duration::from_secs_f64(1.0 / tick_rate));
            let mut render_interval =
                tokio::time::interval(Duration::from_secs_f64(1.0 / frame_rate));

            let _ = event_tx.send(Event::Init);

            loop {
                // Check for cancellation before select
                if cancelled.load(Ordering::Relaxed) {
                    break;
                }

                let event = tokio::select! {
                    _ = tick_interval.tick() => Event::Tick,
                    _ = render_interval.tick() => Event::Render,
                    crossterm_event = event_stream.next().fuse() => {
                        match crossterm_event {
                            Some(Ok(event)) => match event {
                                crossterm::event::Event::Key(key)
                                    if key.kind == KeyEventKind::Press => Event::Key(key),
                                crossterm::event::Event::Mouse(mouse) => Event::Mouse(mouse),
                                crossterm::event::Event::Resize(x, y) => Event::Resize(x, y),
                                crossterm::event::Event::FocusLost => Event::FocusLost,
                                crossterm::event::Event::FocusGained => Event::FocusGained,
                                crossterm::event::Event::Paste(s) => Event::Paste(s),
                                _ => continue,
                            },
                            Some(Err(_)) => Event::Error,
                            None => break,
                        }
                    }
                };

                if event_tx.send(event).is_err() {
                    break;
                }
            }
        });

        Ok(())
    }

    /// Exit the terminal: restore normal mode and stop the event loop.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal restoration fails.
    pub fn exit(&mut self) -> Result<()> {
        self.cancelled.store(true, Ordering::Relaxed);
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// Try to receive the next event from the event channel (non-blocking).
    ///
    /// Returns `None` if no event is available.
    pub fn next_event(&mut self) -> Option<Event> {
        self.event_rx.try_recv().ok()
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

    /// Stop the event loop without restoring the terminal.
    ///
    /// Useful when you want to stop event processing but keep the terminal
    /// in TUI mode (e.g., for a brief pause).
    pub fn stop(&mut self) -> Result<()> {
        self.cancelled.store(true, Ordering::Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tui_new() {
        let tui = Tui::new();
        assert!(tui.is_ok());
    }

    #[test]
    fn test_tui_builder() {
        let tui = Tui::new().unwrap().tick_rate(8.0).frame_rate(30.0);
        assert!((tui.tick_rate - 8.0).abs() < f64::EPSILON);
        assert!((tui.frame_rate - 30.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_event_channel_creation() {
        let tui = Tui::new().unwrap();
        // Sender and receiver should work
        assert!(tui.event_tx.send(Event::Tick).is_ok());
    }
}
