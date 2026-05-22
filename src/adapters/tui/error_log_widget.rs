//! Error Log Widget for async-reactive TUI.
//!
//! This module provides a dedicated error log widget that displays:
//! - Color-coded errors by type (Network=red, Http=yellow, Waf=red+bold)
//! - Timestamps for each error
//! - Scrolling for long error lists
//! - Configurable maximum errors (default 10)
//!
//! # Architecture
//!
//! Follows the Component pattern from `component.rs`:
//! - `ErrorLogWidget` owns `Vec<ErrorEntry>` and implements Component
//! - Receives `Action::Progress` events to extract and display errors
//! - Renders a dedicated error list widget

use anyhow::Result;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};
use std::time::SystemTime;
use tokio::sync::mpsc::UnboundedSender;

use super::action::Action;
use super::component::Component;
use super::theme::Theme;
use crate::adapters::tui::progress_types::{ErrorEntry, ErrorType, ScrapeProgress};

/// Default maximum number of errors to display
pub const DEFAULT_MAX_ERRORS: usize = 10;

/// Error log widget for displaying color-coded errors with scrolling.
///
/// Renders errors with:
/// - Network errors: Red
/// - HTTP errors: Yellow
/// - WAF blocked: Red + Bold
/// - Other errors: White
///
/// Supports scrolling when errors exceed visible area.
///
/// Owns its error data and updates from `Action::Progress` events
/// (extracts errors from `ScrapeProgress::Failed` events).
#[derive(Debug)]
pub struct ErrorLogWidget {
    /// Owned error entries (no lifetime param needed)
    errors: Vec<ErrorEntry>,
    /// Maximum number of errors to display
    max_errors: usize,
    /// Auto-scroll to bottom when new errors arrive
    auto_scroll: bool,
    /// Current scroll offset (for manual scrolling)
    scroll_offset: usize,
    /// Channel sender for dispatching actions
    action_tx: Option<UnboundedSender<Action>>,
}

impl ErrorLogWidget {
    /// Create a new empty error log widget.
    ///
    /// Errors are added dynamically via `Action::Progress` events
    /// through the Component system.
    #[must_use]
    pub fn new() -> Self {
        Self {
            errors: Vec::new(),
            max_errors: DEFAULT_MAX_ERRORS,
            auto_scroll: true,
            scroll_offset: 0,
            action_tx: None,
        }
    }

    /// Set maximum number of errors to display.
    ///
    /// # Example
    /// ```ignore
    /// let widget = ErrorLogWidget::new().with_max_errors(100);
    /// ```
    #[must_use]
    pub fn with_max_errors(mut self, max: usize) -> Self {
        self.max_errors = max;
        self
    }

    /// Set auto-scroll behavior.
    ///
    /// When enabled (default), the widget automatically scrolls to show
    /// the most recent errors. When disabled, the user can manually scroll.
    ///
    /// # Example
    /// ```ignore
    /// let widget = ErrorLogWidget::new().with_auto_scroll(true);
    /// ```
    #[must_use]
    pub fn with_auto_scroll(mut self, auto_scroll: bool) -> Self {
        self.auto_scroll = auto_scroll;
        if auto_scroll {
            self.scroll_offset = 0; // Reset to bottom when auto-scroll is enabled
        }
        self
    }

    /// Get styled content for an error entry based on error type.
    ///
    /// Returns styled spans with appropriate colors:
    /// - Network: Red
    /// - HTTP: Yellow
    /// - WafBlocked: Red + Bold
    /// - Others: White
    fn style_error_entry(entry: &ErrorEntry) -> Line<'_> {
        // Format timestamp as HH:MM:SS
        let time_str = format_time(entry.timestamp);

        // Get icon based on error type
        let (icon, icon_style) = match &entry.error_type {
            ErrorType::Network => ("🌐", Style::default().fg(Theme::error())),
            ErrorType::Http(_) => ("📡", Style::default().fg(Theme::warning())),
            ErrorType::WafBlocked(_) => (
                "🛡️",
                Style::default()
                    .fg(Theme::error())
                    .add_modifier(Modifier::BOLD),
            ),
            ErrorType::Parse(_) => ("🔍", Style::default().fg(Theme::parse_error())),
            ErrorType::Timeout => ("⏱️", Style::default().fg(Theme::warning())),
            ErrorType::Connection => ("🔗", Style::default().fg(Theme::error())),
            ErrorType::Other => ("⚠️", Style::default().fg(Theme::text())),
        };

        // Get message color based on error type
        let message_style = match &entry.error_type {
            ErrorType::Network => Style::default().fg(Theme::error()),
            ErrorType::Http(_) => Style::default().fg(Theme::warning()),
            ErrorType::WafBlocked(_) => Style::default()
                .fg(Theme::error())
                .add_modifier(Modifier::BOLD),
            ErrorType::Parse(_) => Style::default().fg(Theme::parse_error()),
            ErrorType::Timeout => Style::default().fg(Theme::warning()),
            ErrorType::Connection => Style::default().fg(Theme::error()),
            ErrorType::Other => Style::default().fg(Theme::text()),
        };

        // Truncate URL if too long
        let url = if entry.url.len() > 40 {
            format!("{}...", &entry.url[..37])
        } else {
            entry.url.clone()
        };

        Line::from(vec![
            Span::styled(icon, icon_style),
            Span::raw(" "),
            Span::styled(time_str, Style::default().fg(Theme::text_subtle())),
            Span::raw(" "),
            Span::styled(url, Style::default().fg(Theme::accent())),
            Span::raw(" -> "),
            Span::styled(&entry.message, message_style),
        ])
    }

    /// Render the error log widget.
    ///
    /// # Arguments
    ///
    /// * `frame` - Ratatui frame to render into
    /// * `area` - Available rectangle for the widget
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let error_count = self.errors.len();

        // Build title with error count and scroll hint
        let title = if error_count > self.max_errors {
            format!("Errors ({}/{}) (j/k scroll)", self.max_errors, error_count)
        } else {
            format!("Errors ({})", error_count)
        };

        let block = Block::default().borders(Borders::ALL).title(title.as_str());

        if error_count == 0 {
            let para = Paragraph::new("No errors encountered")
                .style(Style::default().fg(Theme::text_subtle()))
                .block(block);
            frame.render_widget(para, area);
            return;
        }

        // Calculate which errors to display based on scroll state
        let display_errors = if self.auto_scroll {
            // Auto-scroll: show most recent errors
            self.errors
                .iter()
                .rev()
                .take(self.max_errors)
                .collect::<Vec<_>>()
        } else {
            // Manual scroll: apply scroll offset
            let start = self.scroll_offset.min(error_count.saturating_sub(1));
            let end = (start + self.max_errors).min(error_count);
            self.errors[start..end].iter().collect::<Vec<_>>()
        };

        // Create list items with styled entries
        let items: Vec<ListItem> = display_errors
            .iter()
            .map(|entry| {
                let line = Self::style_error_entry(entry);
                ListItem::new(line)
            })
            .collect();

        // Create list widget and render
        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    /// Handle up scroll event (move view up)
    pub fn scroll_up(&mut self) {
        if !self.auto_scroll && self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    /// Handle down scroll event (move view down)
    pub fn scroll_down(&mut self) {
        let max_offset = self.errors.len().saturating_sub(self.max_errors);
        if !self.auto_scroll && self.scroll_offset < max_offset {
            self.scroll_offset += 1;
        }
    }

    /// Toggle auto-scroll mode
    pub fn toggle_auto_scroll(&mut self) {
        self.auto_scroll = !self.auto_scroll;
        if self.auto_scroll {
            self.scroll_offset = 0;
        }
    }
}

impl Default for ErrorLogWidget {
    fn default() -> Self {
        Self::new()
    }
}

/// Implement the Component trait for ErrorLogWidget.
///
/// This wires the error log into the reactive App architecture:
/// - `register_action_handler` stores the action channel
/// - `update` extracts errors from `Action::Progress(ScrapeProgress::Failed)` events
/// - `draw` renders the error list using existing render logic
/// - Key events are handled by ProgressWidget, not ErrorLogWidget
impl Component for ErrorLogWidget {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(tx);
        Ok(())
    }

    fn init(&mut self, _area: ratatui::layout::Size) -> Result<()> {
        Ok(())
    }

    fn handle_key_event(&mut self, _key: crossterm::event::KeyEvent) -> Result<Option<Action>> {
        // Key events handled by ProgressWidget
        Ok(None)
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        if let Action::Progress(ScrapeProgress::Failed { url, error }) = action {
            self.errors.push(ErrorEntry {
                timestamp: SystemTime::now(),
                url,
                error_type: error.error_type(),
                message: error.message(),
            });
        }
        Ok(None)
    }

    fn draw(&mut self, f: &mut Frame, rect: Rect) -> Result<()> {
        self.render(f, rect);
        Ok(())
    }
}

/// Helper to format SystemTime as HH:MM:SS
fn format_time(timestamp: SystemTime) -> String {
    use chrono::{DateTime, Utc};
    let dt: DateTime<Utc> = timestamp.into();
    dt.format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn sample_errors() -> Vec<ErrorEntry> {
        vec![
            ErrorEntry {
                timestamp: SystemTime::now(),
                url: "https://example.com/page1".to_string(),
                error_type: ErrorType::WafBlocked("Cloudflare".to_string()),
                message: "WAF blocked (Cloudflare)".to_string(),
            },
            ErrorEntry {
                timestamp: SystemTime::now(),
                url: "https://example.com/page2".to_string(),
                error_type: ErrorType::Network,
                message: "Connection refused".to_string(),
            },
            ErrorEntry {
                timestamp: SystemTime::now(),
                url: "https://example.com/page3".to_string(),
                error_type: ErrorType::Http(404),
                message: "404 Not Found".to_string(),
            },
            ErrorEntry {
                timestamp: SystemTime::now(),
                url: "https://example.com/page4".to_string(),
                error_type: ErrorType::Timeout,
                message: "Request timeout".to_string(),
            },
            ErrorEntry {
                timestamp: SystemTime::now(),
                url: "https://example.com/page5".to_string(),
                error_type: ErrorType::Other,
                message: "Unknown error".to_string(),
            },
        ]
    }

    #[test]
    fn test_error_log_widget_new() {
        let widget = ErrorLogWidget::new();
        assert_eq!(widget.max_errors, DEFAULT_MAX_ERRORS);
        assert!(widget.errors.is_empty());
    }

    #[test]
    fn test_error_log_widget_with_max_errors() {
        let widget = ErrorLogWidget::new().with_max_errors(10);
        assert_eq!(widget.max_errors, 10);
    }

    #[test]
    fn test_error_log_widget_empty() {
        let widget = ErrorLogWidget::new();
        assert!(widget.errors.is_empty());
    }

    #[test]
    fn test_style_error_entry_waf_blocked() {
        let entry = ErrorEntry {
            timestamp: SystemTime::now(),
            url: "https://example.com".to_string(),
            error_type: ErrorType::WafBlocked("Cloudflare".to_string()),
            message: "Blocked".to_string(),
        };

        let line = ErrorLogWidget::style_error_entry(&entry);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_style_error_entry_network() {
        let entry = ErrorEntry {
            timestamp: SystemTime::now(),
            url: "https://example.com".to_string(),
            error_type: ErrorType::Network,
            message: "Connection refused".to_string(),
        };

        let line = ErrorLogWidget::style_error_entry(&entry);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_style_error_entry_http() {
        let entry = ErrorEntry {
            timestamp: SystemTime::now(),
            url: "https://example.com".to_string(),
            error_type: ErrorType::Http(500),
            message: "Internal Server Error".to_string(),
        };

        let line = ErrorLogWidget::style_error_entry(&entry);
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_format_time() {
        let now = SystemTime::now();
        let result = format_time(now);
        assert!(result.len() >= 6);
    }

    #[test]
    fn test_default_max_errors_constant() {
        assert_eq!(DEFAULT_MAX_ERRORS, 10);
    }
}
