//! Progress Widget for async-reactive TUI.
//!
//! This module provides the progress display widget that shows:
//! - Overall progress bar with percentage
//! - Current URL being processed
//! - Error count summary
//! - ETA calculation
//!
//! # Architecture
//!
//! Follows the Component pattern from `component.rs`:
//! - `ProgressWidget` owns `ProgressState` and implements Component
//! - Receives `Action::Progress` events to update state
//! - Renders progress bars, URL list, and error panels
//!
//! ## Example Integration
//!
//! ```no_run
//! use rust_scraper::adapters::tui::{App, AppMode, Header, ProgressWidget};
//! use url::Url;
//!
//! # async fn example() -> anyhow::Result<()> {
//! let urls = vec![Url::parse("https://example.com")?];
//! let mut app = App::new(AppMode::Progress)?
//!     .with_component(Header::new(AppMode::Progress))
//!     .with_component(ProgressWidget::new(&urls));
//! let _ = app.run().await;
//! # Ok(())
//! # }
//! ```

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph},
    Frame,
};
use tokio::sync::mpsc::UnboundedSender;
use url::Url;

use super::action::Action;
use super::component::Component;
use super::theme::Theme;
use crate::adapters::tui::{ErrorType, ProgressState, ScrapeStatus};
use std::time::{Instant, SystemTime};

/// Visual feedback icons for progress indication.
///
/// Provides spinner frames for animated activity indicator and
/// throbber frames for a pulsing effect. Handles animation timing
/// internally.
#[derive(Debug, Clone)]
pub struct ProgressIcons {
    /// Spinner animation frames (circular)
    spinner_frames: Vec<&'static str>,
    /// Throbber animation frames (pulse/glow)
    throbber_frames: Vec<&'static str>,
    /// Current frame index
    current_frame: usize,
    /// Last update instant
    last_update: Instant,
    /// Frame interval (default: 100ms between frames)
    frame_interval: Duration,
}

type Duration = std::time::Duration;

impl ProgressIcons {
    /// Create new ProgressIcons with default animations.
    #[must_use]
    pub fn new() -> Self {
        Self {
            spinner_frames: vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            throbber_frames: vec!["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"],
            current_frame: 0,
            last_update: Instant::now(),
            frame_interval: Duration::from_millis(100),
        }
    }

    /// Get current spinner frame, advancing animation if interval elapsed.
    pub fn spinner(&mut self) -> &'static str {
        let now = Instant::now();
        if now.duration_since(self.last_update) >= self.frame_interval {
            self.current_frame = (self.current_frame + 1) % self.spinner_frames.len();
            self.last_update = now;
        }
        self.spinner_frames[self.current_frame]
    }

    /// Get current throbber frame, advancing animation if interval elapsed.
    pub fn throbber(&mut self) -> &'static str {
        let now = Instant::now();
        if now.duration_since(self.last_update) >= self.frame_interval {
            self.current_frame = (self.current_frame + 1) % self.throbber_frames.len();
            self.last_update = now;
        }
        self.throbber_frames[self.current_frame]
    }

    /// Set custom frame interval for animation.
    ///
    /// # Example
    /// ```no_run
    /// let mut icons = ProgressIcons::new();
    /// icons.set_frame_interval(std::time::Duration::from_millis(50));
    /// ```
    pub fn set_frame_interval(&mut self, interval: Duration) {
        self.frame_interval = interval;
    }

    /// Reset animation to first frame.
    pub fn reset(&mut self) {
        self.current_frame = 0;
        self.last_update = Instant::now();
    }
}

impl Default for ProgressIcons {
    fn default() -> Self {
        Self::new()
    }
}

/// Progress widget for real-time scraping progress display.
///
/// Renders:
/// - Title bar with scraper icon
/// - Progress bar with percentage and ETA
/// - URL list showing per-URL status
/// - Error count panel
///
/// Follows the Component pattern: owns its state, updates from actions,
/// and renders in the draw phase.
#[derive(Debug, Clone)]
pub struct ProgressWidget {
    /// Owned progress state (no lifetime param needed)
    state: ProgressState,
    /// Animation icons for visual feedback
    icons: ProgressIcons,
    /// Whether to show detailed error panel
    show_errors: bool,
    /// Max errors to display
    max_errors: usize,
    /// Channel sender for dispatching actions
    action_tx: Option<UnboundedSender<Action>>,
}

impl ProgressWidget {
    /// Create a new progress widget from a list of URLs.
    ///
    /// Initializes the internal `ProgressState` with the given URLs.
    #[must_use]
    pub fn new(urls: &[Url]) -> Self {
        let url_strings: Vec<String> = urls.iter().map(|u| u.to_string()).collect();
        Self {
            state: ProgressState::new(url_strings),
            icons: ProgressIcons::new(),
            show_errors: true,
            max_errors: 10,
            action_tx: None,
        }
    }

    /// Set whether to show the error panel (default: true).
    #[must_use]
    pub fn with_errors(mut self, show: bool) -> Self {
        self.show_errors = show;
        self
    }

    /// Set maximum number of errors to display (default: 10).
    #[must_use]
    pub fn with_max_errors(mut self, max: usize) -> Self {
        self.max_errors = max;
        self
    }

    /// Get mutable reference to icons for animation control.
    #[must_use]
    pub fn icons_mut(&mut self) -> &mut ProgressIcons {
        &mut self.icons
    }

    /// Render the progress widget.
    ///
    /// Layout:
    /// ```text
    /// ┌────────────────────────────────────────────────────────┐
    /// │ 🕷️ Scraping Progress                          q: Quit │
    /// ├────────────────────────────────────────────────────────┤
    /// │ ████████████░░░░░░░░░░ 40% (2/5)   Est: 12s         │
    /// ├────────────────────────────────────────────────────────┤
    /// │ URL Status                        Chars               │
    /// │ ✅ example.com/1           Completed  1234             │
    /// │ 🔄 example.com/2           Fetching  -                │
    /// │ ⏳ example.com/3           Pending   -                │
    /// ├────────────────────────────────────────────────────────┤
    /// │ Errors (2)                                             │
    /// │ ⚠️ 12:34:56 example.com/3 → HTTP 404                  │
    /// └────────────────────────────────────────────────────────┘
    /// ```
    ///
    /// # Arguments
    ///
    /// * `frame` - Ratatui frame to render into
    /// * `area` - Available rectangle for the entire widget
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .constraints([
                Constraint::Length(3),                                    // Title bar
                Constraint::Length(3),                                    // Progress bar
                Constraint::Min(0),                                       // URL list
                Constraint::Length(if self.show_errors { 4 } else { 1 }), // Errors or spacer
                Constraint::Length(3),                                    // Footer
            ])
            .split(area);

        // Ensure we have at least 5 chunks
        if chunks.len() < 5 {
            return; // Not enough space, skip render
        }

        // 1. Title bar
        self.render_title(frame, chunks[0]);

        // 2. Progress bar + percentage + ETA
        self.render_progress_bar(frame, chunks[1]);

        // 3. URL list
        self.render_url_list(frame, chunks[2]);

        // 4. Error panel
        if self.show_errors {
            self.render_errors(frame, chunks[3]);
        } else {
            // Empty spacer
            let block = Block::default().borders(Borders::NONE);
            frame.render_widget(block, chunks[3]);
        }

        // 5. Footer
        self.render_footer(frame, chunks[4]);
    }

    /// Render title bar.
    fn render_title(&self, frame: &mut Frame, area: Rect) {
        let title = Paragraph::new(Line::from(vec![
            Span::styled("🕷️ ", Style::default().fg(Theme::warning())),
            Span::styled(
                "Scraping Progress",
                Style::default()
                    .fg(Theme::accent())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  q: Quit | Ctrl+C: Stop",
                Style::default().fg(Theme::text_muted()),
            ),
        ]));
        let block = Block::default()
            .borders(Borders::ALL)
            .style(Style::default().fg(Theme::text()));
        frame.render_widget(title.block(block), area);
    }

    /// Render progress bar with percentage and ETA.
    fn render_progress_bar(&mut self, frame: &mut Frame, area: Rect) {
        let percent = self.state.percentage();
        let processed = self.state.completed + self.state.failed;
        let total = self.state.total;

        // Format label: "40% (2/5)  Est: 12s"
        let percentage_str = format!("{:3.0}%", percent);
        let count_str = format!("({}/{})", processed, total);
        let eta_str = match self.state.eta_seconds {
            Some(secs) => {
                if secs == 0 && processed < total {
                    "Est: calculating...".to_string()
                } else if secs == 0 {
                    "Est: done".to_string()
                } else if secs < 60 {
                    format!("Est: {}s", secs)
                } else {
                    let mins = secs / 60;
                    let secs_rem = secs % 60;
                    format!("Est: {}m {}s", mins, secs_rem)
                }
            },
            None => "Est: --".to_string(),
        };

        // Build label with spinner if still processing
        let label = if processed < total {
            format!("{} {} {}", percentage_str, count_str, eta_str)
        } else {
            format!("{} {} ✅", percentage_str, count_str)
        };

        // Choose color based on state
        let gauge_color = if self.state.failed > 0 && self.state.failed < self.state.total {
            Theme::warning() // partial failure
        } else if self.state.failed == self.state.total {
            Theme::error() // all failed
        } else {
            Theme::success() // success or in progress
        };

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL).title("Progress"))
            .gauge_style(
                Style::default()
                    .fg(gauge_color)
                    .bg(Theme::background())
                    .add_modifier(Modifier::BOLD),
            )
            .percent(percent as u16)
            .label(&label);

        frame.render_widget(gauge, area);
    }

    /// Render the per-URL status list.
    fn render_url_list(&self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::{List, ListItem};

        let block = Block::default().borders(Borders::ALL).title("URL Status");

        // Build list items (up to visible height)
        let items: Vec<ListItem> = self
            .state
            .urls
            .iter()
            .map(|url_state| {
                let icon = url_state.status.icon();
                let status_text = match url_state.status {
                    ScrapeStatus::Completed => {
                        if let Some(chars) = url_state.chars {
                            format!("Completed  {} chars", chars)
                        } else {
                            "Completed".to_string()
                        }
                    },
                    ScrapeStatus::Failed => {
                        if let Some(ref err) = url_state.error {
                            format!("Failed  {}", err.message())
                        } else {
                            "Failed".to_string()
                        }
                    },
                    _ => url_state.status.label().to_string(),
                };

                let line = if url_state.status == ScrapeStatus::Pending {
                    Line::from(vec![
                        Span::styled(icon, Style::default().fg(Theme::text_muted())),
                        Span::raw(" "),
                        Span::styled(&url_state.url, Style::default().fg(Theme::text_muted())),
                        Span::raw("  "),
                        Span::styled(status_text, Style::default().fg(Theme::text_muted())),
                    ])
                } else if url_state.status == ScrapeStatus::Fetching
                    || url_state.status == ScrapeStatus::Extracting
                    || url_state.status == ScrapeStatus::Downloading
                {
                    Line::from(vec![
                        Span::styled(
                            icon,
                            Style::default()
                                .fg(Theme::processing())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" "),
                        Span::styled(&url_state.url, Style::default().fg(Theme::text())),
                        Span::raw("  "),
                        Span::styled(status_text, Style::default().fg(Theme::warning())),
                    ])
                } else if url_state.status == ScrapeStatus::Completed {
                    Line::from(vec![
                        Span::styled(icon, Style::default().fg(Theme::success())),
                        Span::raw(" "),
                        Span::styled(&url_state.url, Style::default().fg(Theme::text())),
                        Span::raw("  "),
                        Span::styled(status_text, Style::default().fg(Theme::success())),
                    ])
                } else {
                    // Failed
                    Line::from(vec![
                        Span::styled(icon, Style::default().fg(Theme::error())),
                        Span::raw(" "),
                        Span::styled(&url_state.url, Style::default().fg(Theme::text())),
                        Span::raw("  "),
                        Span::styled(status_text, Style::default().fg(Theme::error())),
                    ])
                };
                ListItem::new(line)
            })
            .collect();

        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    /// Render error panel with latest errors.
    fn render_errors(&self, frame: &mut Frame, area: Rect) {
        use ratatui::widgets::{List, ListItem, Paragraph};

        let error_count = self.state.errors.len();
        let title = format!("Errors ({})", error_count);
        let block = Block::default().borders(Borders::ALL).title(title.as_str());

        if error_count == 0 {
            let para = Paragraph::new("No errors")
                .style(Style::default().fg(Theme::text_muted()))
                .block(block);
            frame.render_widget(para, area);
            return;
        }

        // Show up to max_errors, most recent first (reverse order)
        let display_errors: Vec<_> = self
            .state
            .errors
            .iter()
            .rev()
            .take(self.max_errors)
            .collect();

        let error_items: Vec<ListItem> = display_errors
            .into_iter()
            .map(|entry| {
                // Format time as HH:MM:SS
                let time_str = format_time(entry.timestamp);
                let icon = match entry.error_type {
                    ErrorType::WafBlocked(_) => "🛡️",
                    ErrorType::Network | ErrorType::Connection => "🌐",
                    ErrorType::Http(_) => "📡",
                    ErrorType::Timeout => "⏱️",
                    ErrorType::Parse(_) => "🔍",
                    ErrorType::Other => "⚠️",
                };

                let line = Line::from(vec![
                    Span::styled(icon, Style::default().fg(Theme::warning())),
                    Span::raw(" "),
                    Span::styled(time_str, Style::default().fg(Theme::text_muted())),
                    Span::raw(" "),
                    Span::styled(&entry.url, Style::default().fg(Theme::text())),
                    Span::raw(" → "),
                    Span::styled(&entry.message, Style::default().fg(Theme::error())),
                ]);
                ListItem::new(line)
            })
            .collect();

        let list = List::new(error_items).block(block);
        frame.render_widget(list, area);
    }

    /// Render footer with summary statistics.
    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let completed = self.state.completed;
        let remaining = self
            .state
            .total
            .saturating_sub(completed + self.state.failed);
        let failed = self.state.failed;

        let status_line = format!(
            "📊 {} completed | {} remaining | {} failed",
            completed, remaining, failed
        );

        // ETA on footer as well
        let eta_line = match self.state.eta_seconds {
            Some(secs) if secs > 0 && remaining > 0 => {
                if secs < 60 {
                    format!("⏱ {}s", secs)
                } else {
                    let mins = secs / 60;
                    let s = secs % 60;
                    format!("⏱ {}m {}s", mins, s)
                }
            },
            _ => "⏱ done".to_string(),
        };

        let combined = format!("{}    {}", status_line, eta_line);

        let footer = Paragraph::new(combined)
            .style(Style::default().fg(Theme::text()))
            .block(Block::default().borders(Borders::ALL));

        frame.render_widget(footer, area);
    }
}

/// Implement the Component trait for ProgressWidget.
///
/// This wires the progress view into the reactive App architecture:
/// - `handle_key_event` sends Action::Quit on 'q'
/// - `update` processes Tick (animation) and Action::Progress (state updates)
/// - `draw` delegates to the existing render logic
impl Component for ProgressWidget {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> {
        self.action_tx = Some(tx);
        Ok(())
    }

    fn init(&mut self, _area: ratatui::layout::Size) -> Result<()> {
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> {
        if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q') {
            return Ok(Some(Action::Quit));
        }
        Ok(None)
    }

    fn update(&mut self, action: Action) -> Result<Option<Action>> {
        match action {
            Action::Tick => {
                // Advance animation frame
                self.icons.spinner();
            },
            Action::Progress(progress) => {
                self.state.update(progress);
            },
            _ => {},
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
