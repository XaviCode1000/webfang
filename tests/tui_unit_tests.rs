//! TUI unit tests — Component public contracts.
//!
//! These tests exercise ONLY public interfaces:
//! - Component trait: handle_key_event, update, init
//! - HelpModal construction, key handling
//! - ConfigForm state machine via public methods
//! - centered_rect geometry
//! - App construction (public API only — no dispatch_action)
//!
//! Private method dispatch_action is tested via inline #[cfg(test)] in app.rs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use webfang_tui::tui::action::Action;
use webfang_tui::tui::app::{App, AppResult};
use webfang_tui::tui::component::{AppMode, Component, Header};
use webfang_tui::tui::modal::{centered_rect, HelpModal};
use webfang_tui::tui::ConfigFormState;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

// ===========================================================================
// Group 1: App Initialization (public API only)
// ===========================================================================

#[test]
fn app_initializes_with_selector_mode() {
    let app = App::new(AppMode::Selector).expect("App::new");
    assert_eq!(app.mode, AppMode::Selector);
    assert!(!app.should_quit);
    assert!(!app.should_show_modal);
    assert!(app.components.is_empty());
    assert!(app.modal.is_none());
    assert!(matches!(app.result, AppResult::None));
}

#[test]
fn app_initializes_with_progress_mode() {
    let app = App::new(AppMode::Progress).expect("App::new");
    assert_eq!(app.mode, AppMode::Progress);
}

#[test]
fn app_initializes_with_config_mode() {
    let app = App::new(AppMode::Config).expect("App::new");
    assert_eq!(app.mode, AppMode::Config);
}

#[test]
fn app_result_starts_none() {
    let app = App::new(AppMode::Selector).expect("App::new");
    assert!(matches!(app.result, AppResult::None));
}

#[test]
fn app_with_component_adds_to_list() {
    let app = App::new(AppMode::Selector)
        .expect("App::new")
        .with_component(Header::new(AppMode::Selector))
        .with_component(Header::new(AppMode::Progress));
    assert_eq!(app.components.len(), 2);
}

#[test]
fn app_with_modal_sets_modal() {
    let help = HelpModal::new("Help".into(), vec![]);
    let app = App::new(AppMode::Selector)
        .expect("App::new")
        .with_modal(help);
    assert!(app.modal.is_some());
}

#[test]
fn app_chained_builders() {
    let app = App::new(AppMode::Config)
        .expect("App::new")
        .with_component(Header::new(AppMode::Config))
        .with_component(Header::new(AppMode::Selector))
        .with_modal(HelpModal::new("Help".into(), vec![]));
    assert_eq!(app.mode, AppMode::Config);
    assert_eq!(app.components.len(), 2);
    assert!(app.modal.is_some());
}

// ===========================================================================
// Group 2: HelpModal Escape Handling (Component trait — public port)
// ===========================================================================

#[test]
fn modal_esc_returns_close_action() {
    let mut modal = HelpModal::new("Help".into(), vec![]);
    let action = modal
        .handle_key_event(key(KeyCode::Esc))
        .expect("handle_key_event");
    assert!(matches!(action, Some(Action::CloseModal)));
}

#[test]
fn modal_q_returns_close_action() {
    let mut modal = HelpModal::new("Help".into(), vec![]);
    let action = modal
        .handle_key_event(key(KeyCode::Char('q')))
        .expect("handle_key_event");
    assert!(matches!(action, Some(Action::CloseModal)));
}

#[test]
fn modal_uppercase_q_returns_close_action() {
    let mut modal = HelpModal::new("Help".into(), vec![]);
    let action = modal
        .handle_key_event(key(KeyCode::Char('Q')))
        .expect("handle_key_event");
    assert!(matches!(action, Some(Action::CloseModal)));
}

#[test]
fn modal_unrelated_key_returns_none() {
    let mut modal = HelpModal::new("Help".into(), vec![]);
    let action = modal
        .handle_key_event(key(KeyCode::Char('a')))
        .expect("handle_key_event");
    assert!(action.is_none());
}

#[test]
fn modal_with_multiple_bindings() {
    let bindings = vec![
        ("up/dn".into(), "Navigate".into()),
        ("Space".into(), "Toggle".into()),
        ("Enter".into(), "Confirm".into()),
        ("Esc".into(), "Close".into()),
    ];
    let modal = HelpModal::new("Keybindings".into(), bindings);
    assert_eq!(modal.bindings.len(), 4);
    assert_eq!(modal.title, "Keybindings");
}

#[test]
fn modal_update_tick_is_noop() {
    let mut modal = HelpModal::new("Help".into(), vec![]);
    let action = modal.update(Action::Tick).expect("update");
    assert!(action.is_none());
}

#[test]
fn modal_update_toggle_help_is_noop() {
    let mut modal = HelpModal::new("Help".into(), vec![]);
    let action = modal.update(Action::ToggleHelp).expect("update");
    assert!(action.is_none());
}

// ===========================================================================
// Group 3: centered_rect Geometry
// ===========================================================================

#[test]
fn centered_rect_60x50_returns_centered_region() {
    let area = ratatui::layout::Rect::new(0, 0, 120, 40);
    let rect = centered_rect(60, 50, area);
    // centered_rect(60, 50, area) uses Max(60) for width and Min(50) for height
    assert!(rect.width <= 60, "width should be <= 60: {}", rect.width);
    assert!(rect.height <= 50, "height should be <= 50: {}", rect.height);
    assert!(
        rect.width > 0 && rect.height > 0,
        "should have non-zero size"
    );
    // Should be centered
    let expected_x = (120 - rect.width) / 2;
    let expected_y = (40 - rect.height) / 2;
    assert_eq!(rect.x, expected_x);
    assert_eq!(rect.y, expected_y);
}

#[test]
fn centered_rect_100x100_fills_area() {
    let area = ratatui::layout::Rect::new(0, 0, 80, 24);
    let rect = centered_rect(100, 100, area);
    assert_eq!(rect.width, 80);
    assert_eq!(rect.height, 24);
}

#[test]
fn centered_rect_small_area() {
    let area = ratatui::layout::Rect::new(0, 0, 20, 10);
    let rect = centered_rect(50, 50, area);
    assert!(rect.width >= 8);
    assert!(rect.height >= 4);
}

#[test]
fn centered_rect_symmetry() {
    let area = ratatui::layout::Rect::new(0, 0, 100, 50);
    let rect = centered_rect(40, 30, area);
    // Should be horizontally and vertically centered
    assert_eq!(rect.x, (100 - rect.width) / 2);
    assert_eq!(rect.y, (50 - rect.height) / 2);
}

// ===========================================================================
// Group 4: ConfigForm Key Events (Component trait — public port)
// ===========================================================================

#[test]
fn config_form_q_cancels() {
    let mut form = ConfigFormState::new_default();
    let action = form
        .handle_key_event(key(KeyCode::Char('q')))
        .expect("handle_key_event");
    assert!(form.cancelled);
    assert!(form.is_done());
    assert!(matches!(action, Some(Action::ConfigCancelled)));
}

#[test]
fn config_form_uppercase_q_cancels() {
    let mut form = ConfigFormState::new_default();
    let action = form
        .handle_key_event(key(KeyCode::Char('Q')))
        .expect("handle_key_event");
    assert!(form.cancelled);
    assert!(matches!(action, Some(Action::ConfigCancelled)));
}

#[test]
fn config_form_question_mark_toggles_help() {
    let mut form = ConfigFormState::new_default();
    let action = form
        .handle_key_event(key(KeyCode::Char('?')))
        .expect("handle_key_event");
    assert!(matches!(action, Some(Action::ToggleHelp)));
}

#[test]
fn config_form_unrelated_key_is_active() {
    let mut form = ConfigFormState::new_default();
    let action = form
        .handle_key_event(key(KeyCode::Char('x')))
        .expect("handle_key_event");
    // Non-special key is passed to the form — should remain Active
    assert!(action.is_none());
    assert!(!form.is_done());
}

#[test]
fn config_form_data_is_json_object() {
    let form = ConfigFormState::new_default();
    let data = form.data();
    assert!(data.is_object());
}

#[test]
fn config_form_arrow_navigation_does_not_crash() {
    let mut form = ConfigFormState::new_default();
    for _ in 0..20 {
        form.handle_input(key(KeyCode::Down));
    }
    for _ in 0..20 {
        form.handle_input(key(KeyCode::Up));
    }
    // Arrow navigation should not submit or cancel
    assert!(!form.is_done());
}

#[test]
fn config_form_mark_submitted() {
    let mut form = ConfigFormState::new_default();
    form.mark_submitted();
    assert!(form.submitted);
    assert!(form.is_done());
    assert!(!form.cancelled);
}

#[test]
fn config_form_mark_cancelled() {
    let mut form = ConfigFormState::new_default();
    form.mark_cancelled();
    assert!(form.cancelled);
    assert!(form.is_done());
    assert!(!form.submitted);
}

// ===========================================================================
// Group 5: Action Display & Equality
// ===========================================================================

#[test]
fn action_display_variants() {
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
fn action_display_with_payload() {
    assert_eq!(Action::Resize(80, 24).to_string(), "Resize(80, 24)");
    assert_eq!(Action::Error("test".into()).to_string(), "Error(test)");
    assert_eq!(
        Action::UrlConfirmed(vec!["a".into(), "b".into()]).to_string(),
        "UrlConfirmed(2 urls)"
    );
}

#[test]
fn action_equality() {
    assert_eq!(Action::Tick, Action::Tick);
    assert_ne!(Action::Tick, Action::Render);
    assert_ne!(Action::Quit, Action::ClearScreen);
    assert_eq!(
        Action::UrlConfirmed(vec!["x".into()]),
        Action::UrlConfirmed(vec!["x".into()])
    );
}

// ===========================================================================
// Group 6: AppMode Semantics
// ===========================================================================

#[test]
fn app_mode_all_variants() {
    let _ = AppMode::Selector;
    let _ = AppMode::Progress;
    let _ = AppMode::Config;
}

#[test]
fn app_mode_equality() {
    assert_eq!(AppMode::Selector, AppMode::Selector);
    assert_ne!(AppMode::Selector, AppMode::Progress);
    assert_ne!(AppMode::Progress, AppMode::Config);
    assert_ne!(AppMode::Config, AppMode::Selector);
}
