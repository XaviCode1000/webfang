//! Component trait, Header, and StatusBar for the ratatui Component Architecture.
//!
//! Provides a composable, testable component system where each component:
//! - Registers an action handler for sending actions up the component tree
//! - Initializes itself with the terminal area
//! - Handles raw terminal events (key, mouse)
//! - Updates its state in response to actions
//! - Renders itself to a ratatui Frame

use anyhow::Result;
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::layout::{Rect, Size};
use ratatui::Frame;
use tokio::sync::mpsc::UnboundedSender;

use super::action::Action;
use super::event::Event;
use super::theme::Theme;

/// A composable, interactive UI component for the ratatui architecture.
///
/// Each component has a lifecycle:
/// 1. `register_action_handler` — receives a sender for dispatching actions
/// 2. `init` — called once with the initial terminal size
/// 3. `handle_events` / `handle_key_event` / `handle_mouse_event` — process raw input
/// 4. `update` — processes actions to update component state
/// 5. `draw` — renders the component to the screen
pub trait Component {
    /// Register the action channel sender for dispatching actions.
    ///
    /// Components should store this sender to emit actions (e.g., errors).
    /// Default implementation is a no-op.
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        let _ = tx;
        Ok(())
    }

    /// Initialize the component with the given terminal area.
    ///
    /// Called once after all components have registered their action handlers.
    /// Default implementation is a no-op.
    fn init(&mut self, area: Size) -> Result<()> {
        let _ = area;
        Ok(())
    }

    /// Handle an optional terminal event, returning an action if the event was consumed.
    ///
    /// Default implementation dispatches to `handle_key_event` or `handle_mouse_event`.
    fn handle_events(&mut self, event: Option<Event>) -> Result<Option<Action>> {
        match event {
            Some(Event::Key(key)) => self.handle_key_event(key),
            Some(Event::Mouse(mouse)) => self.handle_mouse_event(mouse),
            _ => Ok(None),
        }
    }

    /// Handle a keyboard event, returning an action if the key was consumed.
    ///
    /// Default implementation ignores the event.
    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        let _ = key;
        Ok(None)
    }

    /// Handle a mouse event, returning an action if the event was consumed.
    ///
    /// Default implementation ignores the event.
    fn handle_mouse_event(&mut self, mouse: MouseEvent) -> Result<Option<Action>> {
        let _ = mouse;
        Ok(None)
    }

    /// Update the component's state in response to an action.
    ///
    /// Returns an optional action that should be dispatched to other components.
    fn update(&mut self, action: Action) -> Result<Option<Action>>;

    /// Render the component to the given frame and area.
    fn draw(&mut self, f: &mut Frame, rect: Rect) -> Result<()>;
}

/// Application mode — determines which component is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    /// URL selection mode
    Selector,
    /// Scraping progress mode
    Progress,
    /// Configuration mode
    Config,
}

/// Header bar showing project name and current mode.
pub struct Header {
    pub mode: AppMode,
    pub status_message: Option<String>,
    action_tx: Option<UnboundedSender<Action>>,
}

impl Header {
    pub fn new(mode: AppMode) -> Self {
        Self {
            mode,
            status_message: None,
            action_tx: None,
        }
    }

    pub fn with_status(mut self, msg: impl Into<String>) -> Self {
        self.status_message = Some(msg.into());
        self
    }
}

impl Component for Header {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(tx);
        Ok(())
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        match action {
            Action::Tick | Action::Render => Ok(None),
            _ => Ok(None),
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> Result<()> {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Paragraph};

        let mode_text = match self.mode {
            AppMode::Selector => "Seleccionar URLs",
            AppMode::Progress => "Scraping",
            AppMode::Config => "Configurar",
        };

        let mut spans = vec![
            Span::styled(" 🕷️ ", Theme::accent()),
            Span::styled("rust_scraper", Theme::text()),
            Span::styled(" │ ", Theme::text_subtle()),
            Span::styled(mode_text, Theme::warning()),
        ];

        if let Some(ref msg) = self.status_message {
            spans.push(Span::styled(" │ ", Theme::text_subtle()));
            spans.push(Span::styled(msg.as_str(), Theme::text_muted()));
        }

        let header = Paragraph::new(Line::from(spans)).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(ratatui::style::Style::new().fg(Theme::surface())),
        );

        frame.render_widget(header, area);
        Ok(())
    }
}

/// Status bar showing keyboard shortcuts and metrics.
pub struct StatusBar {
    pub items: Vec<(String, String)>, // (key, description)
    action_tx: Option<UnboundedSender<Action>>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            action_tx: None,
        }
    }

    pub fn with_items(mut self, items: Vec<(&str, &str)>) -> Self {
        self.items = items
            .into_iter()
            .map(|(k, d)| (k.to_string(), d.to_string()))
            .collect();
        self
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for StatusBar {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(tx);
        Ok(())
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        match action {
            Action::Tick | Action::Render => Ok(None),
            _ => Ok(None),
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> Result<()> {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, BorderType, Paragraph};

        let mut spans = Vec::new();
        for (i, (key, desc)) in self.items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Theme::text_subtle()));
            }
            spans.push(Span::styled(format!("{key}: "), Theme::accent()));
            spans.push(Span::styled(desc.as_str(), Theme::text_muted()));
        }

        let bar = Paragraph::new(Line::from(spans)).block(
            Block::bordered()
                .border_type(BorderType::Rounded)
                .border_style(ratatui::style::Style::new().fg(Theme::surface())),
        );

        frame.render_widget(bar, area);
        Ok(())
    }
}
