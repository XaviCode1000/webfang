//! Modal overlay system for the TUI.
//!
//! Provides a modal overlay that displays on top of the current screen,
//! intercepting input and rendering a centered dialog.
//!
//! # Architecture
//!
//! - `Modal` wraps any Component as an overlay
//! - `HelpModal` is a built-in keybinding help overlay
//! - `centered_rect` calculates a centered rect for any modal content
//!
//! # Usage
//!
//! ```no_run
//! use rust_scraper::adapters::tui::app::App;
//! use rust_scraper::adapters::tui::modal::HelpModal;
//! use rust_scraper::adapters::tui::component::AppMode;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let help = HelpModal::new(
//!     "Ayuda".into(),
//!     vec![("q".into(), "Salir".into())],
//! );
//! let mut app = App::new(AppMode::Selector)?
//!     .with_modal(help);
//! # Ok(())
//! # }
//! ```

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use tokio::sync::mpsc::UnboundedSender;

use super::action::Action;
use super::component::Component;
use super::theme::Theme;

/// A generic modal wrapper that holds a title and a boxed component.
///
/// The modal intercepts input while visible and renders on top
/// of the regular component layout.
pub struct Modal {
    /// Modal title (displayed in the border)
    pub title: String,
    /// The inner component rendered inside the modal
    pub component: Box<dyn Component>,
}

/// Help modal — renders a centered keybinding help overlay.
///
/// Displays a bordered block with the title "Ayuda" and a list
/// of keybinding descriptions. Supports closing via `Esc` or `q`.
pub struct HelpModal {
    /// Modal title
    pub title: String,
    /// List of (key, description) pairs
    pub bindings: Vec<(String, String)>,
    /// Action channel sender
    action_tx: Option<UnboundedSender<Action>>,
}

impl HelpModal {
    /// Create a new HelpModal with a title and keybinding list.
    #[must_use]
    pub fn new(title: String, bindings: Vec<(String, String)>) -> Self {
        Self {
            title,
            bindings,
            action_tx: None,
        }
    }
}

impl Component for HelpModal {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(tx);
        Ok(())
    }

    fn init(&mut self, _area: ratatui::layout::Size) -> Result<()> {
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => {
                return Ok(Some(Action::CloseModal));
            },
            _ => {},
        }
        Ok(None)
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        match action {
            Action::ToggleHelp | Action::CloseModal => {
                // These are handled by App, but we accept them silently
            },
            _ => {},
        }
        Ok(None)
    }

    fn draw(&mut self, f: &mut Frame, rect: Rect) -> Result<()> {
        use ratatui::text::{Line, Span};
        use ratatui::widgets::List;

        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(ratatui::style::Style::default().fg(Theme::accent()));

        let inner = block.inner(rect);

        // Render binding list
        let items: Vec<ratatui::widgets::ListItem> = self
            .bindings
            .iter()
            .map(|(key, desc)| {
                ratatui::widgets::ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {:width$} ", key, width = 8),
                        ratatui::style::Style::default()
                            .fg(Theme::accent())
                            .add_modifier(ratatui::style::Modifier::BOLD),
                    ),
                    Span::styled(
                        desc.as_str(),
                        ratatui::style::Style::default().fg(Theme::text()),
                    ),
                ]))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::NONE)
                .style(ratatui::style::Style::default()),
        );

        f.render_widget(block, rect);

        // Only render list if there's enough space
        if inner.height >= self.bindings.len() as u16 + 2 {
            f.render_widget(list, inner);
        } else {
            let para = Paragraph::new("No hay suficiente espacio para mostrar la ayuda")
                .style(ratatui::style::Style::default().fg(Theme::text_muted()));
            f.render_widget(para, inner);
        }

        Ok(())
    }
}

/// Calculate a centered rectangle for modal overlay placement.
///
/// Uses percentage of the available area:
/// - `percent_x`: horizontal size percentage (0–100)
/// - `percent_y`: vertical size percentage (0–100)
/// - `r`: available area to center within
///
/// Returns a `Rect` centered in `r` with the given size percentages.
#[must_use]
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    // Clamp percentages to valid range
    let pct_x = percent_x.clamp(1, 100);
    let pct_y = percent_y.clamp(1, 100);

    let vertical = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Min(pct_y),
        Constraint::Fill(1),
    ])
    .split(r);

    let horizontal = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Max(pct_x),
        Constraint::Fill(1),
    ])
    .split(vertical[1]);

    horizontal[1]
}
