//! URL Selector State Machine
//!
//! Handles user interaction for URL selection.
//! Separates state from rendering for testability.
//!
//! # Architecture
//!
//! This module implements the state machine for URL selection:
//! - `UrlSelectorState`: Pure state logic (testable without rendering)
//! - `UrlSelector`: Rendering widget (requires ratatui)
//! - `run_selector`: Main event loop (orchestrates state + render)

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect, Size},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use super::action::Action;
use super::app::{App, AppResult};
use super::component::{AppMode, Component, Header, StatusBar};
use super::modal::HelpModal;
use super::theme::Theme;

/// URL selector state (testable without rendering)
///
/// Follows own-borrow-over-clone: stores owned Vec<Url> but provides &Url access
#[derive(Debug, Clone)]
pub struct UrlSelectorState {
    /// All discovered URLs
    urls: Vec<Url>,
    /// Selected indices (parallel to urls)
    selected: Vec<bool>,
    /// Cursor position (index in urls)
    cursor: usize,
    /// Scroll offset (first visible index)
    scroll: usize,
    /// Confirmation mode (showing "Start download?")
    confirm_mode: bool,
    /// Terminal height for scroll calculation
    visible_height: usize,
    /// Channel sender for dispatching actions to the App
    action_tx: Option<UnboundedSender<Action>>,
}

impl UrlSelectorState {
    /// Create new selector state from URLs
    ///
    /// Follows own-borrow-over-clone: accepts `&[Url]`, clones internally
    #[must_use]
    pub fn new(urls: &[Url]) -> Self {
        let selected = vec![false; urls.len()];
        Self {
            urls: urls.to_vec(),
            selected,
            cursor: 0,
            scroll: 0,
            confirm_mode: false,
            visible_height: 10, // Default, updated during render/init
            action_tx: None,
        }
    }

    /// Move cursor up
    #[inline]
    pub fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            // Auto-scroll if cursor goes above visible area
            if self.cursor < self.scroll {
                self.scroll = self.cursor;
            }
        }
    }

    /// Move cursor down
    #[inline]
    pub fn cursor_down(&mut self) {
        if self.cursor < self.urls.len().saturating_sub(1) {
            self.cursor += 1;
            // Auto-scroll if cursor goes below visible area
            if self.cursor >= self.scroll + self.visible_height {
                self.scroll += 1;
            }
        }
    }

    /// Toggle selection at current cursor position
    #[inline]
    pub fn toggle_selection(&mut self) {
        if self.cursor < self.selected.len() {
            self.selected[self.cursor] = !self.selected[self.cursor];
        }
    }

    /// Select all URLs
    #[inline]
    pub fn select_all(&mut self) {
        self.selected.fill(true);
    }

    /// Deselect all URLs
    #[inline]
    pub fn deselect_all(&mut self) {
        self.selected.fill(false);
    }

    /// Enter confirmation mode
    #[inline]
    pub fn enter_confirm_mode(&mut self) {
        self.confirm_mode = true;
    }

    /// Exit confirmation mode
    #[inline]
    pub fn exit_confirm_mode(&mut self) {
        self.confirm_mode = false;
    }

    /// Get selected URLs as owned Vec
    ///
    /// Follows own-borrow-over-clone: returns owned Vec because caller needs to own the data
    #[must_use]
    pub fn get_selected_urls(&self) -> Vec<Url> {
        self.urls
            .iter()
            .enumerate()
            .filter(|(i, _)| self.selected.get(*i).copied().unwrap_or(false))
            .map(|(_, url)| url.clone())
            .collect()
    }

    /// Check if any URL is selected
    #[must_use]
    #[inline]
    pub fn has_selections(&self) -> bool {
        self.selected.iter().any(|&s| s)
    }

    /// Get count of selected URLs
    #[must_use]
    #[inline]
    pub fn selected_count(&self) -> usize {
        self.selected.iter().filter(|&&s| s).count()
    }

    /// Get total URL count
    #[must_use]
    #[inline]
    pub fn total_count(&self) -> usize {
        self.urls.len()
    }

    /// Get URL at index (borrowed)
    #[must_use]
    pub fn get_url(&self, index: usize) -> Option<&Url> {
        self.urls.get(index)
    }

    /// Check if index is selected
    #[must_use]
    #[inline]
    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.get(index).copied().unwrap_or(false)
    }

    /// Update visible height (called during render)
    #[inline]
    pub fn set_visible_height(&mut self, height: usize) {
        self.visible_height = height;
    }

    /// Get cursor position
    #[must_use]
    #[inline]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Get scroll offset
    #[must_use]
    #[inline]
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Check if in confirmation mode
    #[must_use]
    #[inline]
    pub fn is_confirming(&self) -> bool {
        self.confirm_mode
    }
}

/// URL Selector widget (rendering only)
///
/// Follows clean architecture: rendering logic separated from state
pub struct UrlSelector<'a> {
    state: &'a UrlSelectorState,
}

impl<'a> UrlSelector<'a> {
    /// Create new selector widget from state
    #[must_use]
    #[inline]
    pub fn new(state: &'a UrlSelectorState) -> Self {
        Self { state }
    }

    /// Render the selector UI
    ///
    /// # Arguments
    ///
    /// * `frame` - Ratatui frame to render into
    /// * `area` - Available rectangle for rendering
    pub fn render(&self, frame: &mut Frame, area: Rect) {
        // Update visible height in state for scroll calculation
        // Note: We need a mutable reference, but we only have &self
        // This is a design trade-off - scroll calculation uses cached height

        let chunks = Layout::default()
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Min(0),    // List
                Constraint::Length(3), // Footer
            ])
            .split(area);

        // Title bar
        let title = Paragraph::new("🕷️ URL Selector - Space: Select, Enter: Download, q: Quit")
            .style(
                Style::default()
                    .fg(Theme::accent())
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(title, chunks[0]);

        // URL List
        let visible_count = chunks[1].height as usize;
        let items: Vec<ListItem> = self
            .state
            .urls
            .iter()
            .enumerate()
            .skip(self.state.scroll)
            .take(visible_count)
            .map(|(i, url)| {
                let checkbox = if self.state.selected[i] { "✅" } else { "⬜" };
                let cursor = if i == self.state.cursor { "▶ " } else { "  " };
                let style = if i == self.state.cursor {
                    Style::default()
                        .fg(Theme::highlight())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                let text = format!("{}{} {}", cursor, checkbox, url.as_str());
                ListItem::new(Line::from(Span::styled(text, style)))
            })
            .collect();

        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(format!(
            "URLs ({}/{})",
            self.state.selected_count(),
            self.state.total_count()
        )));
        frame.render_widget(list, chunks[1]);

        // Footer with status/confirmation
        let footer_text = if self.state.confirm_mode {
            "🚀 Start download? (Y/N)"
        } else {
            &format!(
                "📊 {} selected ({} total) | ↑↓: Navigate | Space: Toggle | A: All | D: None",
                self.state.selected_count(),
                self.state.total_count()
            )
        };

        let footer = Paragraph::new(footer_text.to_string())
            .style(Style::default().fg(Theme::text()))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);
    }
}

/// Implement the Component trait for UrlSelectorState.
///
/// This wires the URL selector into the reactive App architecture:
/// - `handle_key_event` processes keys and returns Actions
/// - `draw` delegates to the existing `UrlSelector` rendering widget
/// - `update` handles resize events for scroll calculation
impl Component for UrlSelectorState {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(tx);
        Ok(())
    }

    fn init(&mut self, area: Size) -> Result<()> {
        self.visible_height = area.height as usize;
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        match key.code {
            // Navigation
            KeyCode::Up => self.cursor_up(),
            KeyCode::Down => self.cursor_down(),

            // Selection toggles (local navigation, no action)
            KeyCode::Char(' ') => self.toggle_selection(),
            KeyCode::Char('a') | KeyCode::Char('A') => self.select_all(),
            KeyCode::Char('d') | KeyCode::Char('D') => self.deselect_all(),

            // Enter confirmation mode
            KeyCode::Enter if self.has_selections() => {
                self.enter_confirm_mode();
            },

            // Confirm — convert selected URLs to strings and send action
            KeyCode::Char('y') | KeyCode::Char('Y') if self.confirm_mode => {
                let urls: Vec<String> = self
                    .get_selected_urls()
                    .into_iter()
                    .map(|u| u.to_string())
                    .collect();
                return Ok(Some(Action::UrlConfirmed(urls)));
            },

            // Cancel confirmation
            KeyCode::Char('n') | KeyCode::Char('N') if self.confirm_mode => {
                self.exit_confirm_mode();
            },
            KeyCode::Esc if self.confirm_mode => {
                self.exit_confirm_mode();
            },

            // Show help overlay
            KeyCode::Char('?') => {
                return Ok(Some(Action::ToggleHelp));
            },

            // Quit without selection
            KeyCode::Esc | KeyCode::Char('q') => {
                return Ok(Some(Action::UrlCancelled));
            },

            // No-op for other keys
            _ => {},
        }
        Ok(None)
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        match action {
            Action::Resize(_, h) => {
                self.visible_height = h as usize;
            },
            Action::Tick | Action::Render => {},
            _ => {},
        }
        Ok(None)
    }

    fn draw(&mut self, f: &mut Frame, rect: Rect) -> Result<()> {
        // Update visible height so auto-scroll works correctly
        self.visible_height = rect.height as usize;
        let selector = UrlSelector::new(self);
        selector.render(f, rect);
        Ok(())
    }
}

/// Run URL selector interactively using the reactive App architecture.
///
/// Constructs an `App` with Header, UrlSelector, and StatusBar components,
/// runs the event loop, and returns the selected URLs.
///
/// # Arguments
///
/// * `urls` - Slice of discovered URLs to select from
///
/// # Returns
///
/// Vector of selected URLs (owned). Returns an empty vector if cancelled.
///
/// # Errors
///
/// Returns an error if terminal setup or the event loop fails.
///
/// # Example
///
/// ```no_run
/// use url::Url;
/// use rust_scraper::adapters::tui;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let urls = vec![
///     Url::parse("https://example.com/1")?,
///     Url::parse("https://example.com/2")?,
/// ];
/// let selected = tui::run_selector(&urls).await?;
/// # Ok(())
/// # }
/// ```
pub async fn run_selector(urls: &[Url]) -> Result<Vec<Url>> {
    let help_bindings: Vec<(String, String)> = vec![
        ("↑↓".into(), "Navegar".into()),
        ("Space".into(), "Seleccionar/Deseleccionar".into()),
        ("Enter".into(), "Confirmar URLs".into()),
        ("a/A".into(), "Seleccionar todo".into()),
        ("d/D".into(), "Deseleccionar todo".into()),
        ("?".into(), "Mostrar ayuda".into()),
        ("Esc/q".into(), "Cancelar".into()),
    ];

    let mut app = App::new(AppMode::Selector)?
        .with_component(Header::new(AppMode::Selector))
        .with_component(UrlSelectorState::new(urls))
        .with_component(StatusBar::new().with_items(vec![
            ("↑↓", "Navegar"),
            ("Space", "Seleccionar"),
            ("Enter", "Confirmar"),
            ("Esc/q", "Cancelar"),
        ]))
        .with_modal(HelpModal::new(
            "Ayuda — Selector de URLs".into(),
            help_bindings,
        ));

    match app.run().await? {
        AppResult::Urls(urls) => Ok(urls
            .into_iter()
            .filter_map(|s| Url::parse(&s).ok())
            .collect()),
        _ => Ok(vec![]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_urls() -> Vec<Url> {
        vec![
            Url::parse("https://example.com/1").unwrap(),
            Url::parse("https://example.com/2").unwrap(),
            Url::parse("https://example.com/3").unwrap(),
        ]
    }

    #[test]
    fn test_url_selector_state_creation() {
        let urls = test_urls();
        let state = UrlSelectorState::new(&urls);

        assert_eq!(state.total_count(), 3);
        assert_eq!(state.selected_count(), 0);
        assert_eq!(state.cursor(), 0);
        assert_eq!(state.scroll(), 0);
        assert!(!state.is_confirming());
        assert!(!state.has_selections());
    }

    #[test]
    fn test_cursor_movement() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);

        // Move down
        state.cursor_down();
        assert_eq!(state.cursor(), 1);

        state.cursor_down();
        assert_eq!(state.cursor(), 2);

        // Can't go beyond last
        state.cursor_down();
        assert_eq!(state.cursor(), 2);

        // Move up
        state.cursor_up();
        assert_eq!(state.cursor(), 1);

        state.cursor_up();
        assert_eq!(state.cursor(), 0);

        // Can't go before first
        state.cursor_up();
        assert_eq!(state.cursor(), 0);
    }

    #[test]
    fn test_toggle_selection() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);

        // Initially none selected
        assert!(!state.has_selections());
        assert_eq!(state.selected_count(), 0);

        // Toggle first (cursor at 0)
        state.toggle_selection();
        assert!(state.has_selections());
        assert_eq!(state.selected_count(), 1);
        assert!(state.is_selected(0));

        // Toggle again (deselect)
        state.toggle_selection();
        assert!(!state.has_selections());
        assert_eq!(state.selected_count(), 0);
    }

    #[test]
    fn test_select_all() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);

        state.select_all();

        assert!(state.has_selections());
        assert_eq!(state.selected_count(), 3);
        assert!(state.is_selected(0));
        assert!(state.is_selected(1));
        assert!(state.is_selected(2));
    }

    #[test]
    fn test_deselect_all() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);

        // Select some
        state.select_all();
        assert_eq!(state.selected_count(), 3);

        // Deselect all
        state.deselect_all();
        assert_eq!(state.selected_count(), 0);
        assert!(!state.has_selections());
    }

    #[test]
    fn test_get_selected_urls() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);

        // Select first and third
        state.selected[0] = true;
        state.selected[2] = true;

        let selected = state.get_selected_urls();
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].as_str(), "https://example.com/1");
        assert_eq!(selected[1].as_str(), "https://example.com/3");
    }

    #[test]
    fn test_cursor_down_with_scroll() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);
        state.set_visible_height(2); // Only 2 visible at a time

        // Move past visible area
        state.cursor_down(); // cursor=1, scroll=0
        assert_eq!(state.cursor(), 1);
        assert_eq!(state.scroll(), 0);

        state.cursor_down(); // cursor=2, should trigger scroll
        assert_eq!(state.cursor(), 2);
        assert_eq!(state.scroll(), 1); // Scroll to keep cursor visible
    }

    #[test]
    fn test_cursor_up_with_scroll() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);
        state.set_visible_height(2);
        state.scroll = 1;
        state.cursor = 2;

        // Move up into scroll area
        state.cursor_up();
        assert_eq!(state.cursor(), 1);
        assert_eq!(state.scroll(), 1);

        state.cursor_up();
        assert_eq!(state.cursor(), 0);
        assert_eq!(state.scroll(), 0); // Scroll adjusted to keep cursor visible
    }

    #[test]
    fn test_confirmation_mode() {
        let urls = test_urls();
        let mut state = UrlSelectorState::new(&urls);

        assert!(!state.is_confirming());

        state.enter_confirm_mode();
        assert!(state.is_confirming());

        state.exit_confirm_mode();
        assert!(!state.is_confirming());
    }

    #[test]
    fn test_get_url() {
        let urls = test_urls();
        let state = UrlSelectorState::new(&urls);

        assert_eq!(state.get_url(0).unwrap().as_str(), "https://example.com/1");
        assert_eq!(state.get_url(1).unwrap().as_str(), "https://example.com/2");
        assert!(state.get_url(3).is_none());
    }
}
