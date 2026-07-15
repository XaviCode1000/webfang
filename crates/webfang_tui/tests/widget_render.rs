//! TUI widget rendering tests using ratatui::backend::TestBackend.
//!
//! These tests verify that widgets can be instantiated and rendered
//! to a headless backend without panicking. They test the rendering
//! pipeline in isolation from the terminal.

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use url::Url;
use webfang_tui::tui::{
    AppMode, Component, ErrorLogWidget, Header, ProgressWidget, StatusBar, UrlSelectorState,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_urls(n: usize) -> Vec<Url> {
    (0..n)
        .map(|i| Url::parse(&format!("https://example.com/page/{i}")).unwrap())
        .collect()
}

fn make_test_terminal() -> Terminal<TestBackend> {
    let backend = TestBackend::new(80, 24);
    Terminal::new(backend).unwrap()
}

// ===========================================================================
// Header Widget Tests
// ===========================================================================

#[test]
fn header_renders_without_panic() {
    let mut terminal = make_test_terminal();
    let mut header = Header::new(AppMode::Progress);

    terminal
        .draw(|f| {
            let area = f.area();
            header.draw(f, area).unwrap();
        })
        .unwrap();
}

#[test]
fn header_selector_mode() {
    let mut terminal = make_test_terminal();
    let mut header = Header::new(AppMode::Selector);

    terminal
        .draw(|f| {
            header.draw(f, f.area()).unwrap();
        })
        .unwrap();

    // Verify no panic — rendering succeeded
}

#[test]
fn header_config_mode() {
    let mut terminal = make_test_terminal();
    let mut header = Header::new(AppMode::Config);

    terminal
        .draw(|f| {
            header.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

#[test]
fn header_with_status_message() {
    let mut terminal = make_test_terminal();
    let mut header = Header::new(AppMode::Progress).with_status("Testing 123");

    terminal
        .draw(|f| {
            header.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

// ===========================================================================
// StatusBar Widget Tests
// ===========================================================================

#[test]
fn status_bar_renders_without_panic() {
    let mut terminal = make_test_terminal();
    let mut bar = StatusBar::new().with_items(vec![
        ("q", "Quit"),
        ("j/k", "Navigate"),
        ("Enter", "Select"),
    ]);

    terminal
        .draw(|f| {
            bar.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

#[test]
fn status_bar_empty() {
    let mut terminal = make_test_terminal();
    let mut bar = StatusBar::new();

    terminal
        .draw(|f| {
            bar.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

// ===========================================================================
// ProgressWidget Tests
// ===========================================================================

#[test]
fn progress_widget_renders_without_panic() {
    let mut terminal = make_test_terminal();
    let urls = make_urls(3);
    let mut widget = ProgressWidget::new(&urls);

    terminal
        .draw(|f| {
            widget.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

#[test]
fn progress_widget_custom_config() {
    let mut terminal = make_test_terminal();
    let urls = make_urls(5);
    let mut widget = ProgressWidget::new(&urls)
        .with_errors(false)
        .with_max_errors(20);

    terminal
        .draw(|f| {
            widget.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

#[test]
fn progress_widget_single_url() {
    let mut terminal = make_test_terminal();
    let urls = make_urls(1);
    let mut widget = ProgressWidget::new(&urls);

    terminal
        .draw(|f| {
            widget.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

#[test]
fn progress_widget_many_urls() {
    let mut terminal = make_test_terminal();
    let urls = make_urls(50);
    let mut widget = ProgressWidget::new(&urls);

    terminal
        .draw(|f| {
            widget.draw(f, f.area()).unwrap();
        })
        .unwrap();
}

// ===========================================================================
// ErrorLogWidget Tests
// ===========================================================================

#[test]
fn error_log_widget_renders_empty() {
    let mut terminal = make_test_terminal();
    let mut widget = ErrorLogWidget::new();

    terminal
        .draw(|f| {
            widget.render(f, f.area());
        })
        .unwrap();
}

#[test]
fn error_log_widget_with_custom_config() {
    let mut terminal = make_test_terminal();
    let mut widget = ErrorLogWidget::new()
        .with_max_errors(5)
        .with_auto_scroll(false);

    terminal
        .draw(|f| {
            widget.render(f, f.area());
        })
        .unwrap();
}

// ===========================================================================
// UrlSelectorState Tests (state logic, no rendering)
// ===========================================================================

#[test]
fn url_selector_state_creation() {
    let urls = make_urls(5);
    let state = UrlSelectorState::new(&urls);
    assert_eq!(state.total_count(), 5);
    assert_eq!(state.selected_count(), 0);
}

#[test]
fn url_selector_toggle_and_navigate() {
    let urls = make_urls(5);
    let mut state = UrlSelectorState::new(&urls);

    state.toggle_selection(); // index 0
    assert!(state.is_selected(0));

    state.cursor_down();
    state.cursor_down();
    state.toggle_selection(); // index 2
    assert!(state.is_selected(2));

    let selected = state.get_selected_urls();
    assert_eq!(selected.len(), 2);
}

#[test]
fn url_selector_select_all_deselect_all() {
    let urls = make_urls(10);
    let mut state = UrlSelectorState::new(&urls);

    state.select_all();
    assert_eq!(state.selected_count(), 10);

    state.deselect_all();
    assert_eq!(state.selected_count(), 0);
}

#[test]
fn url_selector_scroll_behavior() {
    let urls = make_urls(20);
    let mut state = UrlSelectorState::new(&urls);
    state.set_visible_height(5);

    // Move cursor past visible area
    for _ in 0..10 {
        state.cursor_down();
    }
    assert!(state.scroll() > 0);
}

#[test]
fn url_selector_empty_list() {
    let state = UrlSelectorState::new(&[]);
    assert_eq!(state.total_count(), 0);
    assert_eq!(state.selected_count(), 0);
}

// ===========================================================================
// Small terminal size edge case
// ===========================================================================

#[test]
fn widget_renders_in_small_terminal() {
    let backend = TestBackend::new(20, 5);
    let mut terminal = Terminal::new(backend).unwrap();
    let urls = make_urls(3);
    let mut widget = ProgressWidget::new(&urls);

    terminal
        .draw(|f| {
            widget.draw(f, f.area()).unwrap();
        })
        .unwrap();
}
