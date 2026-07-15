//! Progress View for async-reactive TUI.
//!
//! This module provides the main progress TUI view that displays:
//! - Real-time scraping progress with per-URL status
//! - Error log with color-coded errors
//! - ETA and completion statistics
//!
//! Uses the reactive App + Component architecture from `app.rs` and `component.rs`.
//!
//! # Architecture
//!
//! Components composing the progress view:
//! - `Header`: Shows "Scraping" mode indicator
//! - `ProgressWidget`: Main progress display (owns ProgressState, handles updates)
//! - `ErrorLogWidget`: Dedicated error list display
//! - `StatusBar`: Keyboard shortcuts
//!
//! A background bridge task converts `ScrapeProgress` channel events
//! into `Action::Progress` actions for the component system.
//!
//! # Usage
//!
//! ```no_run
//! use webfang::adapters::tui::run_progress_view;
//! use url::Url;
//! use tokio::sync::mpsc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let urls = vec![Url::parse("https://example.com/1")?];
//! let (tx, rx) = mpsc::channel(100);
//! run_progress_view(rx, &urls).await;
//! # Ok(())
//! # }
//! ```

use std::io;
use tokio::sync::mpsc;
use url::Url;

use crate::tui::{
    app::App,
    component::{AppMode, Header, StatusBar},
    modal::HelpModal,
    progress_types::ScrapeProgress,
    ErrorLogWidget, ProgressWidget,
};

/// Run the progress view TUI using the reactive Component architecture.
///
/// Constructs an `App` with:
/// - `Header` showing "Scraping" mode
/// - `ProgressWidget` for progress bars and URL list
/// - `ErrorLogWidget` for error display
/// - `StatusBar` with keyboard shortcut hints
///
/// A background bridge task converts `ScrapeProgress` events from the
/// channel into `Action::Progress` actions.
///
/// # Arguments
///
/// * `progress_rx` - Receiver for progress events from the scraper
/// * `urls` - List of URLs being scraped (for initial state)
///
/// # Errors
///
/// Returns `io::Error` if terminal setup fails.
pub async fn run_progress_view(
    progress_rx: mpsc::Receiver<ScrapeProgress>,
    urls: &[Url],
) -> io::Result<()> {
    let help_bindings: Vec<(String, String)> = vec![
        ("?".into(), "Mostrar ayuda".into()),
        ("q".into(), "Salir".into()),
    ];

    let mut app = App::new(AppMode::Progress)
        .map_err(|e| io::Error::other(format!("Error al crear app: {e}")))?
        .with_component(Header::new(AppMode::Progress))
        .with_component(ProgressWidget::new(urls))
        .with_component(ErrorLogWidget::new())
        .with_component(StatusBar::new().with_items(vec![("q", "Salir")]))
        .with_progress_bridge(progress_rx)
        .with_modal(HelpModal::new("Ayuda — Progreso".into(), help_bindings));

    let _ = app.run().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::progress_types::{ScrapeError, ScrapeStatus};

    fn sample_urls() -> Vec<Url> {
        vec![
            Url::parse("https://example.com/1").unwrap(),
            Url::parse("https://example.com/2").unwrap(),
        ]
    }

    #[tokio::test]
    async fn test_progress_state_updates() {
        let url_strings: Vec<String> = sample_urls().iter().map(|u| u.to_string()).collect();
        let mut state = crate::tui::ProgressState::new(url_strings);

        // Test Started event
        state.update(ScrapeProgress::Started {
            url: "https://example.com/1".to_string(),
        });
        assert_eq!(state.urls[0].status, ScrapeStatus::Fetching);

        // Test Completed event
        state.update(ScrapeProgress::Completed {
            url: "https://example.com/1".to_string(),
            chars: 1000,
        });
        assert_eq!(state.completed, 1);
        assert_eq!(state.urls[0].status, ScrapeStatus::Completed);

        // Test Failed event
        state.update(ScrapeProgress::Started {
            url: "https://example.com/2".to_string(),
        });
        state.update(ScrapeProgress::Failed {
            url: "https://example.com/2".to_string(),
            error: ScrapeError::Network("connection refused".to_string()),
        });
        assert_eq!(state.failed, 1);
        assert_eq!(state.urls[1].status, ScrapeStatus::Failed);
    }

    #[test]
    fn test_progress_state_percentage() {
        let url_strings = vec![
            "https://example.com/1".to_string(),
            "https://example.com/2".to_string(),
        ];
        let mut state = crate::tui::ProgressState::new(url_strings);

        // Initially 0%
        assert_eq!(state.percentage(), 0.0);

        // Complete one URL (50%)
        state.update(ScrapeProgress::Completed {
            url: "https://example.com/1".to_string(),
            chars: 100,
        });
        assert!((state.percentage() - 50.0).abs() < 0.1);

        // Fail another (100%)
        state.update(ScrapeProgress::Failed {
            url: "https://example.com/2".to_string(),
            error: ScrapeError::Other("error".to_string()),
        });
        assert_eq!(state.percentage(), 100.0);
    }
}
