//! App struct — component orchestrator for the TUI.
//!
//! The App manages the lifecycle of all components:
//! 1. Creates the Tui terminal
//! 2. Registers action handlers on all components
//! 3. Initializes all components with the terminal size
//! 4. Runs the event loop (handle events → process actions → render)
//! 5. Returns the result (selected URLs, config, or none)
//!
//! # Architecture
//!
//! The App implements a reactive architecture:
//! - Events flow IN from the Tui event loop
//! - Events are converted to Actions
//! - Actions are dispatched to all components via `update()`
//! - Components may produce new Actions in response
//! - The App handles certain Actions directly (Quit, UrlConfirmed, etc.)

use anyhow::Result;
use ratatui::prelude::*;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use super::action::Action;
use super::component::{AppMode, Component};
use super::event::Event;
use super::progress_types::ScrapeProgress;
use super::tui_terminal::Tui;

/// Result of running the App.
///
/// Different modes return different result types.
pub enum AppResult {
    /// URLs selected by the user
    Urls(Vec<String>),
    /// Configuration values from the form
    Config(Option<serde_json::Value>),
    /// No result (cancel or error)
    None,
}

/// Main application orchestrator.
///
/// Manages component lifecycle and the event → action → render loop.
///
/// # Example
///
/// ```no_run
/// use rust_scraper::adapters::tui::{App, AppMode, Header, StatusBar, Component};
///
/// # async fn example() -> anyhow::Result<()> {
/// let mut app = App::new(AppMode::Selector)?
///     .with_component(Header::new(AppMode::Selector));
/// let result = app.run().await?;
/// # Ok(())
/// # }
/// ```
pub struct App {
    /// All registered components (drawn in order)
    pub components: Vec<Box<dyn Component>>,
    /// Sender for dispatching actions
    pub action_tx: UnboundedSender<Action>,
    /// Receiver for consuming actions
    action_rx: UnboundedReceiver<Action>,
    /// Whether the app should quit
    pub should_quit: bool,
    /// Current application mode
    pub mode: AppMode,
    /// Result to return when the app exits
    pub result: AppResult,
}

impl App {
    /// Create a new App with the given mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the action channel creation fails (unlikely).
    pub fn new(mode: AppMode) -> Result<Self> {
        let (action_tx, action_rx) = mpsc::unbounded_channel();
        Ok(Self {
            components: Vec::new(),
            action_tx,
            action_rx,
            should_quit: false,
            mode,
            result: AppResult::None,
        })
    }

    /// Add a component to the app.
    ///
    /// Components are drawn in the order they are added.
    #[must_use]
    pub fn with_component(mut self, component: impl Component + 'static) -> Self {
        self.components.push(Box::new(component));
        self
    }

    /// Bridge a progress channel to the action system.
    ///
    /// Spawns a background task that polls the `mpsc::Receiver<ScrapeProgress>`
    /// and forwards each event as an `Action::Progress(ScrapeProgress)` action.
    ///
    /// When the channel closes (scraper finished), sends a final
    /// `Action::Progress(ScrapeProgress::Finished)` to signal completion.
    #[must_use]
    pub fn with_progress_bridge(
        self,
        mut progress_rx: tokio::sync::mpsc::Receiver<ScrapeProgress>,
    ) -> Self {
        let action_tx = self.action_tx.clone();
        tokio::spawn(async move {
            while let Some(progress) = progress_rx.recv().await {
                if action_tx.send(Action::Progress(progress)).is_err() {
                    break;
                }
            }
            // Channel closed — send Finished signal
            let _ = action_tx.send(Action::Progress(ScrapeProgress::Finished {
                total: 0,
                successful: 0,
                failed: 0,
            }));
        });
        self
    }

    /// Run the app: enter TUI, process events, render components.
    ///
    /// Returns when the user exits or an action triggers termination.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal setup or rendering fails.
    pub async fn run(&mut self) -> Result<AppResult> {
        let mut tui = Tui::new()?;
        tui.enter()?;

        // Phase 1: Register action handlers on all components
        for component in self.components.iter_mut() {
            component.register_action_handler(self.action_tx.clone())?;
        }

        // Phase 2: Initialize all components with the terminal size
        let size = tui.size()?;
        for component in self.components.iter_mut() {
            component.init(size)?;
        }

        // Phase 3: Main event loop
        loop {
            self.handle_events(&mut tui)?;
            self.handle_actions(&mut tui)?;
            if self.should_quit {
                tui.stop()?;
                break;
            }
        }

        // Phase 4: Cleanup
        tui.exit()?;

        let result = std::mem::replace(&mut self.result, AppResult::None);
        Ok(result)
    }

    /// Process incoming events from the Tui event loop.
    ///
    /// Converts terminal events into actions and dispatches them.
    /// Also forwards events to all components for custom handling.
    fn handle_events(&mut self, tui: &mut Tui) -> Result<()> {
        if let Some(event) = tui.next_event() {
            // Dispatch basic events as actions
            let action_tx = self.action_tx.clone();
            match &event {
                Event::Quit => {
                    let _ = action_tx.send(Action::Quit);
                },
                Event::Tick => {
                    let _ = action_tx.send(Action::Tick);
                },
                Event::Render => {
                    let _ = action_tx.send(Action::Render);
                },
                Event::Resize(w, h) => {
                    let _ = action_tx.send(Action::Resize(*w, *h));
                },
                // Key events are handled by components via handle_events
                Event::Key(_) => {},
                _ => {},
            }

            // Forward events to components for custom handling
            for component in self.components.iter_mut() {
                if let Some(action) = component.handle_events(Some(event.clone()))? {
                    let _ = self.action_tx.send(action);
                }
            }
        }
        Ok(())
    }

    /// Process all pending actions from the action channel.
    ///
    /// Handles system-level actions (Quit, Render, Resize, etc.)
    /// and forwards all actions to components for state updates.
    fn handle_actions(&mut self, tui: &mut Tui) -> Result<()> {
        while let Ok(action) = self.action_rx.try_recv() {
            // Handle system-level actions
            match &action {
                Action::Render => {
                    let action_tx = self.action_tx.clone();
                    tui.draw(|frame| {
                        for component in self.components.iter_mut() {
                            if let Err(e) = component.draw(frame, frame.area()) {
                                let _ = action_tx
                                    .send(Action::Error(format!("Error al dibujar: {}", e)));
                            }
                        }
                    })?;
                },
                Action::Quit => self.should_quit = true,
                Action::ClearScreen => {
                    let _ = tui.terminal.clear();
                },
                Action::Resize(w, h) => {
                    tui.resize(Rect::new(0, 0, *w, *h))?;
                },

                // Result actions — set result and quit
                Action::UrlConfirmed(urls) => {
                    self.result = AppResult::Urls(urls.clone());
                    self.should_quit = true;
                },
                Action::UrlCancelled => {
                    self.result = AppResult::Urls(vec![]);
                    self.should_quit = true;
                },
                Action::ConfigDone(value) => {
                    self.result = AppResult::Config(value.clone());
                    self.should_quit = true;
                },
                Action::ConfigCancelled => {
                    self.result = AppResult::Config(None);
                    self.should_quit = true;
                },

                // Tick and other actions are forwarded to components
                _ => {},
            }

            // Forward all actions to components for state updates
            // Match on reference above avoids moving `action`
            for component in self.components.iter_mut() {
                if let Some(new_action) = component.update(action.clone())? {
                    let _ = self.action_tx.send(new_action);
                }
            }
        }
        Ok(())
    }
}
