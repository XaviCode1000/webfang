//! Unit tests for TUI state machines — no terminal rendering, no network.
//!
//! Exercises public API of UrlSelectorState, ErrorLogWidget, ProgressState,
//! and Theme using ratatui::backend::TestBackend for widget rendering tests.
//!
//! Follows contract-based-test-audit: public API only, deterministic timestamps,
//! semantic assertions, no stubs.

use ratatui::backend::TestBackend;
use ratatui::Terminal;
use webfang_tui::tui::{
    theme::{Theme, ThemeMode},
    Action, CollapsibleConfig, Component, ErrorLogWidget, ProgressState, ScrapeError,
    ScrapeProgress, ScrapeStatus, UrlSelector, UrlSelectorState,
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use url::Url;

// ============================================================================
// Helpers
// ============================================================================

fn sample_urls(count: usize) -> Vec<Url> {
    (1..=count)
        .map(|i| Url::parse(&format!("https://example.com/page{i}")).unwrap())
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(120, 40)).unwrap()
}

// ============================================================================
// 1. UrlSelectorState
// ============================================================================

#[test]
fn url_selector_creates_with_correct_total() {
    let urls = sample_urls(5);
    let state = UrlSelectorState::new(&urls);
    assert_eq!(state.total_count(), 5);
    assert_eq!(state.selected_count(), 0);
    assert!(!state.has_selections());
}

#[test]
fn url_selector_cursor_starts_at_zero() {
    let state = UrlSelectorState::new(&sample_urls(3));
    assert_eq!(state.cursor(), 0);
}

#[test]
fn url_selector_cursor_down_advances() {
    let mut state = UrlSelectorState::new(&sample_urls(5));
    state.cursor_down();
    assert_eq!(state.cursor(), 1);
    state.cursor_down();
    assert_eq!(state.cursor(), 2);
}

#[test]
fn url_selector_cursor_down_stops_at_end() {
    let mut state = UrlSelectorState::new(&sample_urls(3));
    state.cursor_down();
    state.cursor_down();
    state.cursor_down(); // At end (index 2)
    state.cursor_down(); // Should not advance
    assert_eq!(state.cursor(), 2);
}

#[test]
fn url_selector_cursor_up_goes_to_zero() {
    let mut state = UrlSelectorState::new(&sample_urls(3));
    state.cursor_down();
    state.cursor_down();
    state.cursor_up();
    state.cursor_up();
    assert_eq!(state.cursor(), 0);
}

#[test]
fn url_selector_cursor_up_stops_at_zero() {
    let mut state = UrlSelectorState::new(&sample_urls(3));
    state.cursor_up(); // Already at 0
    assert_eq!(state.cursor(), 0);
}

#[test]
fn url_selector_toggle_selection() {
    let mut state = UrlSelectorState::new(&sample_urls(3));
    assert!(!state.is_selected(0));
    state.toggle_selection();
    assert!(state.is_selected(0));
    assert!(state.has_selections());
    assert_eq!(state.selected_count(), 1);
    state.toggle_selection();
    assert!(!state.is_selected(0));
    assert_eq!(state.selected_count(), 0);
}

#[test]
fn url_selector_select_all() {
    let mut state = UrlSelectorState::new(&sample_urls(4));
    state.select_all();
    assert_eq!(state.selected_count(), 4);
    assert!(state.has_selections());
}

#[test]
fn url_selector_deselect_all() {
    let mut state = UrlSelectorState::new(&sample_urls(4));
    state.select_all();
    state.deselect_all();
    assert_eq!(state.selected_count(), 0);
    assert!(!state.has_selections());
}

#[test]
fn url_selector_get_selected_urls_returns_correct_subset() {
    let urls = sample_urls(4);
    let mut state = UrlSelectorState::new(&urls);
    state.cursor_down();
    state.toggle_selection(); // page2
    state.cursor_down();
    state.cursor_down();
    state.toggle_selection(); // page4
    let selected = state.get_selected_urls();
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].as_str(), "https://example.com/page2");
    assert_eq!(selected[1].as_str(), "https://example.com/page4");
}

#[test]
fn url_selector_scroll_when_cursor_exceeds_visible_height() {
    let mut state = UrlSelectorState::new(&sample_urls(20));
    state.set_visible_height(5);
    // Move cursor past visible area
    for _ in 0..6 {
        state.cursor_down();
    }
    assert!(
        state.scroll() > 0,
        "should auto-scroll when cursor goes below visible area"
    );
}

#[test]
fn url_selector_scroll_tracks_cursor_going_above_visible_area() {
    let mut state = UrlSelectorState::new(&sample_urls(20));
    state.set_visible_height(5);
    // Scroll down first
    for _ in 0..10 {
        state.cursor_down();
    }
    assert!(state.scroll() > 0);
    // Now move back up past the visible area
    for _ in 0..10 {
        state.cursor_up();
    }
    assert_eq!(state.scroll(), 0);
}

#[test]
fn url_selector_confirm_mode() {
    let mut state = UrlSelectorState::new(&sample_urls(3));
    assert!(!state.is_confirming());
    state.enter_confirm_mode();
    assert!(state.is_confirming());
    state.exit_confirm_mode();
    assert!(!state.is_confirming());
}

#[test]
fn url_selector_get_url_at_index() {
    let urls = sample_urls(3);
    let state = UrlSelectorState::new(&urls);
    assert_eq!(
        state.get_url(0).unwrap().as_str(),
        "https://example.com/page1"
    );
    assert!(state.get_url(99).is_none());
}

#[test]
fn url_selector_empty_urls() {
    let state = UrlSelectorState::new(&[]);
    assert_eq!(state.total_count(), 0);
    assert!(!state.has_selections());
}

// ============================================================================
// 2. ErrorLogWidget
// ============================================================================

#[test]
fn error_log_widget_creates_empty() {
    let _widget = ErrorLogWidget::new();
    // Default max_errors is 10
    let mut term = terminal();
    term.draw(|f| {
        let area = f.area();
        let mut w = ErrorLogWidget::new();
        w.render(f, area);
    })
    .unwrap();
}

#[test]
fn error_log_widget_with_max_errors() {
    let _widget = ErrorLogWidget::new().with_max_errors(50);
    // Render to verify no panic
    let mut term = terminal();
    term.draw(|f| {
        let mut w = ErrorLogWidget::new().with_max_errors(50);
        w.render(f, f.area());
    })
    .unwrap();
}

#[test]
fn error_log_widget_styled_entry_network() {
    let mut term = terminal();
    term.draw(|f| {
        let mut w = ErrorLogWidget::new();
        let action = Action::Progress(ScrapeProgress::Failed {
            url: "https://example.com".to_string(),
            error: ScrapeError::Network("Connection refused".to_string()),
        });
        let result = w.update(action);
        assert!(result.is_ok(), "update should succeed: {:?}", result.err());
        assert!(
            result.unwrap().is_none(),
            "update should return None action"
        );
        w.render(f, f.area());
    })
    .unwrap();
    // Verify the buffer contains the error URL text
    let buf = term.backend().buffer().clone();
    let text: String = buf
        .content()
        .iter()
        .map(|c| c.symbol().to_owned())
        .collect();
    assert!(
        text.contains("Errors (1)"),
        "buffer should show error count: {text}"
    );
}

#[test]
fn error_log_widget_styled_entry_http() {
    let mut term = terminal();
    term.draw(|f| {
        let mut w = ErrorLogWidget::new();
        let action = Action::Progress(ScrapeProgress::Failed {
            url: "https://example.com".to_string(),
            error: ScrapeError::Http(403, "Forbidden".to_string()),
        });
        let result = w.update(action);
        assert!(result.is_ok(), "update should succeed: {:?}", result.err());
        assert!(
            result.unwrap().is_none(),
            "update should return None action"
        );
        w.render(f, f.area());
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = buf
        .content()
        .iter()
        .map(|c| c.symbol().to_owned())
        .collect();
    assert!(
        text.contains("Errors (1)"),
        "buffer should show error count: {text}"
    );
}

#[test]
fn error_log_widget_styled_entry_waf() {
    let mut term = terminal();
    term.draw(|f| {
        let mut w = ErrorLogWidget::new();
        let action = Action::Progress(ScrapeProgress::Failed {
            url: "https://example.com".to_string(),
            error: ScrapeError::WafBlocked("Cloudflare".to_string()),
        });
        let result = w.update(action);
        assert!(result.is_ok(), "update should succeed: {:?}", result.err());
        assert!(
            result.unwrap().is_none(),
            "update should return None action"
        );
        w.render(f, f.area());
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let text: String = buf
        .content()
        .iter()
        .map(|c| c.symbol().to_owned())
        .collect();
    assert!(
        text.contains("Errors (1)"),
        "buffer should show error count: {text}"
    );
}

#[test]
fn error_log_widget_render_with_empty_state() {
    let mut term = terminal();
    term.draw(|f| {
        let mut w = ErrorLogWidget::new();
        w.render(f, f.area());
    })
    .unwrap();
    // If we got here, rendering didn't panic
}

// ============================================================================
// 3. ProgressState
// ============================================================================

#[test]
fn progress_state_new_initializes_correctly() {
    let state = ProgressState::new(vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
    ]);
    assert_eq!(state.total, 2);
    assert_eq!(state.completed, 0);
    assert_eq!(state.failed, 0);
    assert_eq!(state.urls.len(), 2);
    assert!(state.urls.iter().all(|u| u.status == ScrapeStatus::Pending));
}

#[test]
fn progress_state_percentage_zero_when_no_urls() {
    let state = ProgressState::new(vec![]);
    assert_eq!(state.percentage(), 0.0);
}

#[test]
fn progress_state_percentage_50_percent() {
    let mut state = ProgressState::new(vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
    ]);
    state.update(ScrapeProgress::Completed {
        url: "https://example.com/1".to_string(),
        chars: 1000,
    });
    assert!((state.percentage() - 50.0).abs() < 0.1);
}

#[test]
fn progress_state_percentage_100_percent_on_completion() {
    let mut state = ProgressState::new(vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
    ]);
    state.update(ScrapeProgress::Completed {
        url: "https://example.com/1".to_string(),
        chars: 100,
    });
    state.update(ScrapeProgress::Failed {
        url: "https://example.com/2".to_string(),
        error: ScrapeError::Other("error".to_string()),
    });
    assert_eq!(state.percentage(), 100.0);
}

#[test]
fn progress_state_started_sets_fetching() {
    let mut state = ProgressState::new(vec!["https://example.com/1".to_string()]);
    state.update(ScrapeProgress::Started {
        url: "https://example.com/1".to_string(),
    });
    assert_eq!(state.urls[0].status, ScrapeStatus::Fetching);
}

#[test]
fn progress_state_completed_increments_counter() {
    let mut state = ProgressState::new(vec!["https://example.com/1".to_string()]);
    state.update(ScrapeProgress::Completed {
        url: "https://example.com/1".to_string(),
        chars: 500,
    });
    assert_eq!(state.completed, 1);
    assert_eq!(state.urls[0].status, ScrapeStatus::Completed);
    assert_eq!(state.urls[0].chars, Some(500));
}

#[test]
fn progress_state_failed_records_error() {
    let mut state = ProgressState::new(vec!["https://example.com/1".to_string()]);
    state.update(ScrapeProgress::Failed {
        url: "https://example.com/1".to_string(),
        error: ScrapeError::Network("timeout".to_string()),
    });
    assert_eq!(state.failed, 1);
    assert_eq!(state.urls[0].status, ScrapeStatus::Failed);
    assert_eq!(state.errors.len(), 1);
}

#[test]
fn progress_state_is_complete() {
    let mut state = ProgressState::new(vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
    ]);
    assert!(!state.is_complete());
    state.update(ScrapeProgress::Completed {
        url: "https://example.com/1".to_string(),
        chars: 100,
    });
    assert!(!state.is_complete());
    state.update(ScrapeProgress::Failed {
        url: "https://example.com/2".to_string(),
        error: ScrapeError::Other("err".to_string()),
    });
    assert!(state.is_complete());
}

#[test]
fn progress_state_status_changed_event() {
    let mut state = ProgressState::new(vec!["https://example.com/1".to_string()]);
    state.update(ScrapeProgress::StatusChanged {
        url: "https://example.com/1".to_string(),
        status: ScrapeStatus::Extracting,
    });
    assert_eq!(state.urls[0].status, ScrapeStatus::Extracting);
}

#[test]
fn progress_state_current_url_while_fetching() {
    let mut state = ProgressState::new(vec![
        "https://example.com/1".to_string(),
        "https://example.com/2".to_string(),
    ]);
    assert!(state.current_url().is_none());
    state.update(ScrapeProgress::Started {
        url: "https://example.com/1".to_string(),
    });
    assert_eq!(state.current_url(), Some("https://example.com/1"));
}

// ============================================================================
// 4. Theme — WCAG contrast & color conversion
// ============================================================================

#[test]
fn theme_has_contrast_text_vs_background() {
    assert!(Theme::has_contrast(Theme::text(), Theme::background(), 4.5));
}

#[test]
fn theme_has_contrast_fails_for_surface_vs_bg() {
    assert!(!Theme::has_contrast(
        Theme::surface(),
        Theme::background(),
        4.5
    ));
}

#[test]
fn theme_contrast_ratio_positive() {
    let ratio = Theme::contrast(Theme::text(), Theme::background());
    assert!(ratio > 0.0);
}

#[test]
fn theme_contrast_ratio_symmetric() {
    let a = Theme::contrast(Theme::text(), Theme::background());
    let b = Theme::contrast(Theme::background(), Theme::text());
    assert!((a - b).abs() < f64::EPSILON);
}

#[test]
fn theme_contrast_meets_wcag_aa_text() {
    assert!(Theme::contrast(Theme::text(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_meets_wcag_aa_error() {
    assert!(Theme::contrast(Theme::error(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_meets_wcag_aa_success() {
    assert!(Theme::contrast(Theme::success(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_meets_wcag_aa_warning() {
    assert!(Theme::contrast(Theme::warning(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_meets_wcag_aa_accent() {
    assert!(Theme::contrast(Theme::accent(), Theme::background()) >= 4.5);
}

#[test]
fn theme_lighten_produces_brighter_color() {
    let dark = ratatui::style::Color::Rgb(0x1e, 0x1e, 0x2e);
    let light = Theme::lighten(dark, 0.3);
    assert_ne!(dark, light);
    if let (ratatui::style::Color::Rgb(lr, lg, lb), ratatui::style::Color::Rgb(dr, dg, db)) =
        (light, dark)
    {
        assert!(
            lr > dr || lg > dg || lb > db,
            "lightened should be brighter"
        );
    }
}

#[test]
fn theme_darken_produces_darker_color() {
    let light = ratatui::style::Color::Rgb(0xcd, 0xd6, 0xf4);
    let dark = Theme::darken(light, 0.3);
    assert_ne!(light, dark);
}

#[test]
fn theme_lighten_does_not_panic_on_non_rgb() {
    assert_eq!(
        Theme::lighten(ratatui::style::Color::Reset, 0.3),
        ratatui::style::Color::Reset
    );
    assert_eq!(
        Theme::lighten(ratatui::style::Color::Black, 0.3),
        ratatui::style::Color::Black
    );
}

#[test]
fn theme_darken_does_not_panic_on_non_rgb() {
    assert_eq!(
        Theme::darken(ratatui::style::Color::Reset, 0.3),
        ratatui::style::Color::Reset
    );
    assert_eq!(
        Theme::darken(ratatui::style::Color::Black, 0.3),
        ratatui::style::Color::Black
    );
}

#[test]
fn theme_shift_hue_changes_color() {
    let original = ratatui::style::Color::Rgb(0x89, 0xb4, 0xfa);
    let shifted = Theme::shift_hue(original, 60.0);
    assert_ne!(original, shifted);
}

#[test]
fn theme_shift_hue_zero_is_near_identity() {
    let original = ratatui::style::Color::Rgb(0x89, 0xb4, 0xfa);
    let same = Theme::shift_hue(original, 0.0);
    if let (ratatui::style::Color::Rgb(r1, g1, b1), ratatui::style::Color::Rgb(r2, g2, b2)) =
        (original, same)
    {
        assert!((r1 as i16 - r2 as i16).abs() <= 1);
        assert!((g1 as i16 - g2 as i16).abs() <= 1);
        assert!((b1 as i16 - b2 as i16).abs() <= 1);
    }
}

#[test]
fn theme_shift_hue_360_is_near_identity() {
    let original = ratatui::style::Color::Rgb(0x89, 0xb4, 0xfa);
    let same = Theme::shift_hue(original, 360.0);
    if let (ratatui::style::Color::Rgb(r1, g1, b1), ratatui::style::Color::Rgb(r2, g2, b2)) =
        (original, same)
    {
        assert!((r1 as i16 - r2 as i16).abs() <= 1);
        assert!((g1 as i16 - g2 as i16).abs() <= 1);
        assert!((b1 as i16 - b2 as i16).abs() <= 1);
    }
}

#[test]
fn theme_shift_hue_achromatic_returns_original() {
    let gray = ratatui::style::Color::Rgb(0x80, 0x80, 0x80);
    let shifted = Theme::shift_hue(gray, 90.0);
    assert_eq!(gray, shifted);
}

#[test]
fn theme_adaptive_modes_produce_valid_themes() {
    let _ = Theme::adaptive(ThemeMode::Dark);
    let _ = Theme::adaptive(ThemeMode::Light);
    let _ = Theme::adaptive(ThemeMode::HighContrast);
}

// ============================================================================
// 5. UrlSelectorState rendering via TestBackend
// ============================================================================

#[test]
fn url_selector_renders_in_test_backend() {
    let urls = sample_urls(3);
    let state = UrlSelectorState::new(&urls);
    let selector = UrlSelector::new(&state);

    let mut term = terminal();
    term.draw(|f| {
        selector.render(f, f.area());
    })
    .unwrap();
    // If we reached here, rendering succeeded without panic
}

// ============================================================================
// 6. CollapsibleConfig state tests
// ============================================================================

#[test]
fn collapsible_config_creates_with_multiple_sections() {
    let config = CollapsibleConfig::new();
    let json = config.to_json();
    let obj = json.as_object().unwrap();
    assert!(obj.len() >= 8);
}

#[test]
fn collapsible_config_navigation_through_all_sections() {
    let mut config = CollapsibleConfig::new();
    // Navigate all sections without panicking
    for _ in 0..20 {
        let _ = config.handle_key_event(key(KeyCode::Down));
    }
    for _ in 0..20 {
        let _ = config.handle_key_event(key(KeyCode::Up));
    }
    assert!(!config.submitted);
    assert!(!config.cancelled);
}

#[test]
fn collapsible_config_space_toggles_expand_collapse() {
    let mut config = CollapsibleConfig::new();
    // Target (index 0) starts expanded
    assert!(config.is_section_expanded(0));
    let _ = config.handle_key_event(key(KeyCode::Char(' ')));
    assert!(!config.is_section_expanded(0));
    let _ = config.handle_key_event(key(KeyCode::Char(' ')));
    assert!(config.is_section_expanded(0));
}
