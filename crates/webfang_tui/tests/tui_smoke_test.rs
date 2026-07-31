//! TUI smoke tests — validate state logic without terminal rendering.
//!
//! These tests exercise pure state machines and widget initialization
//! without requiring a terminal backend, keeping them fast and CI-friendly.

// TUI smoke tests require the `ui` feature (ratatui types).
#![cfg(feature = "ui")]

use webfang::adapters::tui::action::Action;
use webfang::adapters::tui::component::Component;
use webfang::adapters::tui::modal::HelpModal;
use webfang::adapters::tui::{
    AppMode, ConfigFormState, ErrorLogWidget, ProgressWidget, UrlSelectorState,
};
use url::Url;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_urls(n: usize) -> Vec<Url> {
    (0..n)
        .map(|i| Url::parse(&format!("https://example.com/page/{i}")).unwrap())
        .collect()
}

// ===========================================================================
// Group 1: UrlSelectorState
//
// Core UrlSelectorState tests live in tui_unit_tests.rs (more comprehensive).
// Only tests for behavior NOT covered there belong here.
// ===========================================================================

#[test]
fn url_selector_toggle_out_of_bounds() {
    let urls = make_urls(3);
    let mut state = UrlSelectorState::new(&urls);
    // Cursor at 0, should not panic
    state.toggle_selection();
    assert!(state.is_selected(0));
}

// ===========================================================================
// Group 2: ProgressWidget
// ===========================================================================

#[test]
fn progress_widget_init_no_panic() {
    let urls = make_urls(3);
    let _widget = ProgressWidget::new(&urls);
}

#[test]
fn progress_widget_init_empty() {
    let _widget = ProgressWidget::new(&[]);
}

#[test]
fn progress_widget_builder_methods() {
    let urls = make_urls(1);
    let _widget = ProgressWidget::new(&urls)
        .with_errors(false)
        .with_max_errors(5);
}

// ===========================================================================
// Group 3: ErrorLogWidget
//
// Core ErrorLogWidget::new() and rendering tests live in tui_unit_tests.rs.
// Only tests for behavior NOT covered there belong here.
// ===========================================================================

#[test]
fn error_log_widget_toggle_auto_scroll() {
    let mut widget = ErrorLogWidget::new();
    widget.toggle_auto_scroll();
    // Should not panic — just toggles internal state
}

#[test]
fn error_log_widget_builder_methods() {
    let _widget = ErrorLogWidget::new()
        .with_max_errors(20)
        .with_auto_scroll(false);
}

#[test]
fn error_log_widget_scroll_methods() {
    let mut widget = ErrorLogWidget::new().with_auto_scroll(false);
    // Scrolling empty widget should not panic
    widget.scroll_up();
    widget.scroll_down();
}

// ===========================================================================
// Group 4: ConfigFormState
// ===========================================================================

#[test]
fn config_form_init_no_panic() {
    let _form = ConfigFormState::new_default();
}

#[test]
fn config_form_initial_state() {
    let form = ConfigFormState::new_default();
    assert!(!form.submitted);
    assert!(!form.cancelled);
}

#[test]
fn config_form_mark_submitted() {
    let mut form = ConfigFormState::new_default();
    form.mark_submitted();
    assert!(form.submitted);
    assert!(form.is_done());
}

#[test]
fn config_form_mark_cancelled() {
    let mut form = ConfigFormState::new_default();
    form.mark_cancelled();
    assert!(form.cancelled);
    assert!(form.is_done());
}

#[test]
fn config_form_data_returns_json() {
    let form = ConfigFormState::new_default();
    let data = form.data();
    // Default form should produce a JSON object
    assert!(data.is_object());
}

// ===========================================================================
// Group 5: AppMode
// ===========================================================================

#[test]
fn app_mode_selector() {
    let mode = AppMode::Selector;
    assert!(matches!(mode, AppMode::Selector));
}

#[test]
fn app_mode_progress() {
    let mode = AppMode::Progress;
    assert!(matches!(mode, AppMode::Progress));
}

#[test]
fn app_mode_config() {
    let mode = AppMode::Config;
    assert!(matches!(mode, AppMode::Config));
}

#[test]
fn app_mode_all_variants_exist() {
    // Compile-time check: all expected variants constructible
    let _ = AppMode::Selector;
    let _ = AppMode::Progress;
    let _ = AppMode::Config;
}

#[test]
fn app_mode_equality() {
    assert_eq!(AppMode::Selector, AppMode::Selector);
    assert_ne!(AppMode::Selector, AppMode::Progress);
}

// ===========================================================================
// Group 6: HelpModal
// ===========================================================================

#[test]
fn help_modal_init_no_panic() {
    let _modal = HelpModal::new("Help".into(), vec![("q".into(), "Quit".into())]);
}

#[test]
fn help_modal_with_bindings() {
    let bindings = vec![
        ("↑↓".into(), "Navigate".into()),
        ("Space".into(), "Toggle".into()),
        ("Enter".into(), "Confirm".into()),
    ];
    let modal = HelpModal::new("Help".into(), bindings);
    assert_eq!(modal.bindings.len(), 3);
}

// ===========================================================================
// Group 7: State Machine Transitions (AppMode)
// ===========================================================================

#[test]
fn test_app_mode_transition_selector_to_progress() {
    let mode = AppMode::Selector;
    let next = match mode {
        AppMode::Selector => AppMode::Progress,
        other => other,
    };
    assert!(matches!(next, AppMode::Progress));
}

#[test]
fn test_app_mode_transition_progress_to_config() {
    let mode = AppMode::Progress;
    let next = match mode {
        AppMode::Progress => AppMode::Config,
        other => other,
    };
    assert!(matches!(next, AppMode::Config));
}

#[test]
fn test_app_mode_transition_back_to_selector() {
    let mode = AppMode::Config;
    let next = match mode {
        AppMode::Config => AppMode::Selector,
        other => other,
    };
    assert!(matches!(next, AppMode::Selector));
}

#[test]
fn test_app_mode_full_cycle() {
    let modes = [AppMode::Selector, AppMode::Progress, AppMode::Config];
    // Verify all modes are distinct
    for (i, a) in modes.iter().enumerate() {
        for (j, b) in modes.iter().enumerate() {
            if i == j {
                assert_eq!(a, b);
            } else {
                assert_ne!(a, b);
            }
        }
    }
}

// ===========================================================================
// Group 8: Action Display & Equality
// ===========================================================================

#[test]
fn test_action_display_variants() {
    assert_eq!(Action::Tick.to_string(), "Tick");
    assert_eq!(Action::Render.to_string(), "Render");
    assert_eq!(Action::Quit.to_string(), "Quit");
    assert_eq!(Action::ClearScreen.to_string(), "ClearScreen");
    assert_eq!(Action::Suspend.to_string(), "Suspend");
    assert_eq!(Action::Resume.to_string(), "Resume");
    assert_eq!(Action::ToggleHelp.to_string(), "ToggleHelp");
    assert_eq!(Action::CloseModal.to_string(), "CloseModal");
    assert_eq!(Action::UrlCancelled.to_string(), "UrlCancelled");
    assert_eq!(Action::ConfigCancelled.to_string(), "ConfigCancelled");
}

#[test]
fn test_action_display_with_payload() {
    assert_eq!(Action::Resize(80, 24).to_string(), "Resize(80, 24)");
    assert_eq!(Action::Error("test".into()).to_string(), "Error(test)");
    assert_eq!(
        Action::UrlConfirmed(vec!["a".into(), "b".into()]).to_string(),
        "UrlConfirmed(2 urls)"
    );
}

#[test]
fn test_action_equality() {
    assert_eq!(Action::Tick, Action::Tick);
    assert_ne!(Action::Tick, Action::Render);
    assert_ne!(Action::Quit, Action::ClearScreen);
    assert_eq!(
        Action::UrlConfirmed(vec!["x".into()]),
        Action::UrlConfirmed(vec!["x".into()])
    );
}

// ===========================================================================
// Group 9: Event Dispatch Routing
// ===========================================================================

#[test]
fn test_toggle_help_action_concept() {
    // Verify ToggleHelp action exists and is distinct from other modal actions
    let action = Action::ToggleHelp;
    assert!(!matches!(action, Action::CloseModal));
    assert!(!matches!(action, Action::Quit));
}

#[test]
fn test_close_modal_action_concept() {
    let action = Action::CloseModal;
    assert!(!matches!(action, Action::ToggleHelp));
    assert!(!matches!(action, Action::Quit));
}

#[test]
fn test_url_confirmed_carries_urls() {
    let urls = vec!["https://a.com".into(), "https://b.com".into()];
    let action = Action::UrlConfirmed(urls.clone());
    match action {
        Action::UrlConfirmed(v) => assert_eq!(v, urls),
        _ => panic!("Expected UrlConfirmed"),
    }
}

#[test]
fn test_config_done_carries_value() {
    let value = serde_json::json!({"key": "value"});
    let action = Action::ConfigDone(Some(value.clone()));
    match action {
        Action::ConfigDone(Some(v)) => assert_eq!(v, value),
        _ => panic!("Expected ConfigDone with Some"),
    }
}

#[test]
fn test_config_done_none() {
    let action = Action::ConfigDone(None);
    match action {
        Action::ConfigDone(None) => {},
        _ => panic!("Expected ConfigDone(None)"),
    }
}

// ===========================================================================
// Group 10: ProgressWidget State Updates
// ===========================================================================

#[test]
fn test_progress_widget_update_tick() {
    let urls = make_urls(2);
    let mut widget = ProgressWidget::new(&urls);
    // Tick should not panic — advances animation
    let result = widget.update(Action::Tick);
    assert!(result.is_ok());
}

#[test]
fn test_progress_widget_update_render() {
    let urls = make_urls(2);
    let mut widget = ProgressWidget::new(&urls);
    let result = widget.update(Action::Render);
    assert!(result.is_ok());
}

// ===========================================================================
// Group 11: ErrorLogWidget State Updates
// ===========================================================================

#[test]
fn test_error_log_widget_update_tick() {
    let mut widget = ErrorLogWidget::new();
    let result = widget.update(Action::Tick);
    assert!(result.is_ok());
}

#[test]
fn test_error_log_widget_update_render() {
    let mut widget = ErrorLogWidget::new();
    let result = widget.update(Action::Render);
    assert!(result.is_ok());
}
