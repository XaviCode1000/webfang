//! Component trait and App struct for modern TUI architecture.
//!
//! Provides a composable, testable component system for the terminal UI.

use crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use super::theme::Theme;

/// A renderable, interactive UI component.
pub trait Component {
    /// Render the component to the given area.
    fn draw(&self, frame: &mut Frame, area: Rect);

    /// Handle a keyboard event. Returns true if the event was consumed.
    fn handle_key_event(&mut self, _event: KeyEvent) -> bool {
        false // default: ignore
    }
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
}

impl Header {
    pub fn new(mode: AppMode) -> Self {
        Self {
            mode,
            status_message: None,
        }
    }

    pub fn with_status(mut self, msg: impl Into<String>) -> Self {
        self.status_message = Some(msg.into());
        self
    }
}

impl Component for Header {
    fn draw(&self, frame: &mut Frame, area: Rect) {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Paragraph};

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
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(ratatui::style::Style::new().fg(Theme::surface())),
        );

        frame.render_widget(header, area);
    }
}

/// Status bar showing keyboard shortcuts and metrics.
pub struct StatusBar {
    pub items: Vec<(String, String)>, // (key, description)
}

impl StatusBar {
    pub fn new() -> Self {
        Self { items: Vec::new() }
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
    fn draw(&self, frame: &mut Frame, area: Rect) {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Paragraph};

        let mut spans = Vec::new();
        for (i, (key, desc)) in self.items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Theme::text_subtle()));
            }
            spans.push(Span::styled(format!("{}: ", key), Theme::accent()));
            spans.push(Span::styled(desc.as_str(), Theme::text_muted()));
        }

        let bar = Paragraph::new(Line::from(spans)).block(
            Block::bordered()
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(ratatui::style::Style::new().fg(Theme::surface())),
        );

        frame.render_widget(bar, area);
    }
}
