//! App struct — component orchestrator for the TUI.
//!
//! The App manages the lifecycle of all components:
//! 1. Creates the Tui terminal
//! 2. Registers action handlers on all components
//! 3. Initializes all components with the terminal size
//! 4. Runs the **unified event loop** (crosstrem events → tick → actions → render)
//! 5. Returns the result (selected URLs, config, or none)
//!
//! # Architecture
//!
//! The App implements a reactive architecture with a SINGLE event loop
//! that owns the crossterm EventStream, tick interval, and action channel:
//!
//! ```text
//!                 ┌──────────────────────┐
//!                 │     App::run()        │
//!                 │   tokio::select!      │
//!                 │  (biased priority)    │
//!                 │                       │
//!   crossterm ───►│  event_stream.next()  │──► on_event() ──► components
//!   events        │                       │
//!   progress  ───►│  action_rx.recv()     │──► dispatch_action() ──► components
//!   bridge        │                       │
//!   tick      ───►│  tick_interval.tick() │──► Action::Tick ──► components
//!   (250ms)       │                       │
//!                 │  after select:        │
//!                 │  1. drain actions     │
//!                 │  2. draw()            │
//!                 └──────────────────────┘
//! ```
//!
//! No more background event loop task in Tui — App directly manages
//! everything, eliminating the double-channel bounce and resume() race.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{EventStream, KeyEventKind};
use futures::StreamExt;
use ratatui::prelude::*;
use ratatui::widgets::Clear;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::time::interval;

use super::action::Action;
use super::component::{AppMode, Component};
use super::event::Event;
use super::modal::centered_rect;
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
/// Manages component lifecycle and the unified event → action → render loop.
///
/// # Example
///
/// ```no_run
/// use webfang::adapters::tui::{App, AppMode, Header, StatusBar, Component};
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
    /// Optional modal overlay component
    pub modal: Option<Box<dyn Component>>,
    /// Whether a modal is currently visible
    pub should_show_modal: bool,
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
            modal: None,
            should_show_modal: false,
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

    /// Add a modal overlay component to the app.
    ///
    /// The modal will be rendered on top of other components and
    /// intercept events when `should_show_modal` is true.
    #[must_use]
    pub fn with_modal(mut self, modal: impl Component + 'static) -> Self {
        self.modal = Some(Box::new(modal));
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

    /// Run the app: enter TUI, unify event loop, render components.
    ///
    /// The unified event loop uses `tokio::select!` with biased priority:
    /// 1. **crossterm events** (keyboard, mouse, resize) — user input first
    /// 2. **action channel** (progress updates, component feedback)
    /// 3. **tick interval** (250ms heartbeat for animations)
    ///
    /// After each select iteration, remaining actions are drained and
    /// the UI is rendered, guaranteeing responsiveness.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal setup or rendering fails.
    pub async fn run(&mut self) -> Result<AppResult> {
        let mut tui = Tui::new()?;
        tui.enter()?;

        // Phase 1: Register action handlers on all components (and modal if present)
        for component in self.components.iter_mut() {
            component.register_action_handler(self.action_tx.clone())?;
        }
        if let Some(modal) = &mut self.modal {
            modal.register_action_handler(self.action_tx.clone())?;
        }

        // Phase 2: Initialize all components (and modal if present) with terminal size
        let size = tui.size()?;
        for component in self.components.iter_mut() {
            component.init(size)?;
        }
        if let Some(modal) = &mut self.modal {
            modal.init(size)?;
        }

        // Phase 3: Unified event loop
        let mut event_stream = EventStream::new();
        let mut tick_interval = interval(Duration::from_millis(250));
        tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;

                // Priority 1: User input — handle immediately
                Some(Ok(crossterm_event)) = event_stream.next() => {
                    self.on_event(crossterm_event, &mut tui)?;
                }

                // Priority 2: Actions from components or progress bridge
                action = self.action_rx.recv() => {
                    match action {
                        Some(a) => self.dispatch_action(a, &mut tui)?,
                        None => break, // Channel closed
                    }
                }

                // Priority 3: Tick for periodic updates
                _ = tick_interval.tick() => {
                    let _ = self.action_tx.send(Action::Tick);
                }
            }

            // Drain remaining pending actions before rendering
            while let Ok(action) = self.action_rx.try_recv() {
                self.dispatch_action(action, &mut tui)?;
            }

            // Render every iteration
            self.draw(&mut tui)?;

            if self.should_quit {
                break;
            }
        }

        // Phase 4: Cleanup
        tui.exit()?;

        let result = std::mem::replace(&mut self.result, AppResult::None);
        Ok(result)
    }

    /// Handle a single raw terminal event from crossterm.
    ///
    /// - Resize is handled immediately (terminal dimensions updated, action sent)
    /// - Key events are filtered to only press events (ignore release/repeat)
    /// - Events are forwarded to the active modal (if showing) or all components
    fn on_event(&mut self, event: crossterm::event::Event, tui: &mut Tui) -> Result<()> {
        // Handle resize immediately — update terminal and dispatch action
        if let crossterm::event::Event::Resize(w, h) = event {
            tui.resize(Rect::new(0, 0, w, h))?;
            let _ = self.action_tx.send(Action::Resize(w, h));
            return Ok(());
        }

        // Convert crossterm event to our Event enum, filtering key press kind
        let app_event: Option<Event> = match event {
            crossterm::event::Event::Key(key) if key.kind == KeyEventKind::Press => {
                Some(Event::Key(key))
            },
            crossterm::event::Event::Mouse(mouse) => Some(Event::Mouse(mouse)),
            crossterm::event::Event::Paste(s) => Some(Event::Paste(s)),
            crossterm::event::Event::FocusLost => Some(Event::FocusLost),
            crossterm::event::Event::FocusGained => Some(Event::FocusGained),
            _ => None, // Ignore key release/repeat and unknown events
        };

        let Some(app_event) = app_event else {
            return Ok(());
        };

        if self.should_show_modal {
            // When modal is showing, only forward events to the modal component
            if let Some(modal) = &mut self.modal {
                if let Some(action) = modal.handle_events(Some(app_event))? {
                    let _ = self.action_tx.send(action);
                }
            }
        } else {
            // Forward events to all regular components
            for component in self.components.iter_mut() {
                if let Some(action) = component.handle_events(Some(app_event.clone()))? {
                    let _ = self.action_tx.send(action);
                }
            }
        }
        Ok(())
    }

    /// Process a single action.
    ///
    /// Handles system-level actions (Quit, ToggleHelp, etc.) and forwards
    /// every action to all components for state updates.
    fn dispatch_action(&mut self, action: Action, tui: &mut Tui) -> Result<()> {
        // Handle system-level actions
        match &action {
            Action::Quit => self.should_quit = true,
            Action::ClearScreen => {
                let _ = tui.terminal.clear();
            },
            Action::Resize(w, h) => {
                tui.resize(Rect::new(0, 0, *w, *h))?;
            },
            Action::ToggleHelp => {
                self.should_show_modal = !self.should_show_modal;
            },
            Action::CloseModal => {
                self.should_show_modal = false;
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

            // Tick, Render, Suspend, Resume, Error, Progress
            // — forwarded to components below, no system-level handling
            _ => {},
        }

        // Forward all actions to components for state updates
        for component in self.components.iter_mut() {
            if let Some(new_action) = component.update(action.clone())? {
                let _ = self.action_tx.send(new_action);
            }
        }

        // Also forward actions to the modal for state updates
        if let Some(modal) = &mut self.modal {
            if let Some(new_action) = modal.update(action.clone())? {
                let _ = self.action_tx.send(new_action);
            }
        }

        Ok(())
    }

    /// Render all components to the terminal.
    ///
    /// Draws each component in order, then renders the modal overlay
    /// on top if one is active.
    fn draw(&mut self, tui: &mut Tui) -> Result<()> {
        let action_tx = self.action_tx.clone();
        let should_show = self.should_show_modal;

        tui.draw(|frame| {
            for component in self.components.iter_mut() {
                if let Err(e) = component.draw(frame, frame.area()) {
                    let _ = action_tx.send(Action::Error(format!("Error al dibujar: {e}")));
                }
            }

            // Draw modal overlay on top if active
            if should_show {
                if let Some(modal) = &mut self.modal {
                    // Dark overlay background
                    frame.render_widget(Clear, frame.area());

                    // Calculate centered area (60% width, 50% height)
                    let modal_rect = centered_rect(60, 50, frame.area());

                    if let Err(e) = modal.draw(frame, modal_rect) {
                        let _ =
                            action_tx.send(Action::Error(format!("Error al dibujar modal: {e}")));
                    }
                }
            }
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_new_selector_mode() {
        let app = App::new(AppMode::Selector).expect("App::new should not fail");
        assert_eq!(app.mode, AppMode::Selector);
        assert!(!app.should_quit);
        assert!(!app.should_show_modal);
        assert!(app.components.is_empty());
        assert!(app.modal.is_none());
    }

    #[test]
    fn app_new_progress_mode() {
        let app = App::new(AppMode::Progress).expect("App::new should not fail");
        assert_eq!(app.mode, AppMode::Progress);
    }

    #[test]
    fn app_new_config_mode() {
        let app = App::new(AppMode::Config).expect("App::new should not fail");
        assert_eq!(app.mode, AppMode::Config);
    }

    #[test]
    fn app_result_starts_none() {
        let app = App::new(AppMode::Selector).expect("App::new should not fail");
        assert!(matches!(app.result, AppResult::None));
    }

    #[test]
    fn app_with_component_increments_count() {
        let app = App::new(AppMode::Selector)
            .expect("App::new should not fail")
            .with_component(Header::new(AppMode::Selector));
        assert_eq!(app.components.len(), 1);
    }

    #[test]
    fn app_dispatch_quit_sets_should_quit() {
        let mut app = App::new(AppMode::Selector).expect("App::new should not fail");
        let mut tui = Tui::new().expect("Tui::new");
        tui.enter().expect("tui.enter");
        app.dispatch_action(Action::Quit, &mut tui)
            .expect("dispatch");
        assert!(app.should_quit);
        let _ = tui.exit();
    }

    #[test]
    fn app_dispatch_toggle_help_toggles_modal() {
        let mut app = App::new(AppMode::Selector).expect("App::new should not fail");
        let mut tui = Tui::new().expect("Tui::new");
        tui.enter().expect("tui.enter");
        assert!(!app.should_show_modal);
        app.dispatch_action(Action::ToggleHelp, &mut tui)
            .expect("dispatch");
        assert!(app.should_show_modal);
        app.dispatch_action(Action::CloseModal, &mut tui)
            .expect("dispatch");
        assert!(!app.should_show_modal);
        let _ = tui.exit();
    }

    #[test]
    fn app_dispatch_url_confirmed_sets_result_and_quits() {
        let mut app = App::new(AppMode::Selector).expect("App::new should not fail");
        let mut tui = Tui::new().expect("Tui::new");
        tui.enter().expect("tui.enter");
        let urls = vec!["https://a.com".into(), "https://b.com".into()];
        app.dispatch_action(Action::UrlConfirmed(urls.clone()), &mut tui)
            .expect("dispatch");
        assert!(app.should_quit);
        match &app.result {
            AppResult::Urls(v) => assert_eq!(v, &urls),
            other => panic!("Expected Urls, got {:?}", other),
        }
        let _ = tui.exit();
    }

    #[test]
    fn app_dispatch_url_cancelled_sets_empty() {
        let mut app = App::new(AppMode::Selector).expect("App::new should not fail");
        let mut tui = Tui::new().expect("Tui::new");
        tui.enter().expect("tui.enter");
        app.dispatch_action(Action::UrlCancelled, &mut tui)
            .expect("dispatch");
        assert!(app.should_quit);
        match &app.result {
            AppResult::Urls(v) => assert!(v.is_empty()),
            other => panic!("Expected empty Urls, got {:?}", other),
        }
        let _ = tui.exit();
    }

    #[test]
    fn app_dispatch_config_done_sets_config() {
        let mut app = App::new(AppMode::Config).expect("App::new should not fail");
        let mut tui = Tui::new().expect("Tui::new");
        tui.enter().expect("tui.enter");
        let value = serde_json::json!({"key": "value"});
        app.dispatch_action(Action::ConfigDone(Some(value.clone())), &mut tui)
            .expect("dispatch");
        assert!(app.should_quit);
        match &app.result {
            AppResult::Config(Some(v)) => assert_eq!(v, &value),
            other => panic!("Expected Config(Some), got {:?}", other),
        }
        let _ = tui.exit();
    }

    #[test]
    fn app_dispatch_config_cancelled_sets_none() {
        let mut app = App::new(AppMode::Config).expect("App::new should not fail");
        let mut tui = Tui::new().expect("Tui::new");
        tui.enter().expect("tui.enter");
        app.dispatch_action(Action::ConfigCancelled, &mut tui)
            .expect("dispatch");
        assert!(app.should_quit);
        assert!(matches!(app.result, AppResult::Config(None)));
        let _ = tui.exit();
    }
}
