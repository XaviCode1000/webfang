//! TUI unit tests — App lifecycle, Modal dispatch, ConfigForm validation.
//!
//! These tests exercise the public contracts of the TUI component system:
//! App orchestration (action dispatch, modal toggling, result handling),
//! HelpModal escape/close behavior, and ConfigForm key event routing.
//!
//! All tests run without a real terminal — they test state transitions only.

#![cfg(feature = "ui")]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use webfang::adapters::tui::action::Action;
use webfang::adapters::tui::app::{App, AppResult};
use webfang::adapters::tui::component::{AppMode, Component, Header};
use webfang::adapters::tui::config_form::ConfigFormState;
use webfang::adapters::tui::modal::{centered_rect, HelpModal};
use webfang::adapters::tui::tui_terminal::Tui;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

// ===========================================================================
// Group 1: App Initialization
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

// ===========================================================================
// Group 2: App Action Dispatch
// ===========================================================================

#[test]
fn app_dispatch_quit_sets_should_quit() {
    let mut app = App::new(AppMode::Selector).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    app.dispatch_action(Action::Quit, &mut tui)
        .expect("dispatch");
    assert!(app.should_quit);
    let _ = tui.exit();
}

#[test]
fn app_dispatch_toggle_help_toggles_modal_flag() {
    let mut app = App::new(AppMode::Selector).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    assert!(!app.should_show_modal);
    app.dispatch_action(Action::ToggleHelp, &mut tui)
        .expect("dispatch");
    assert!(app.should_show_modal);
    app.dispatch_action(Action::ToggleHelp, &mut tui)
        .expect("dispatch");
    assert!(!app.should_show_modal);
    let _ = tui.exit();
}

#[test]
fn app_dispatch_close_modal_hides_modal() {
    let mut app = App::new(AppMode::Selector).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    app.should_show_modal = true;
    app.dispatch_action(Action::CloseModal, &mut tui)
        .expect("dispatch");
    assert!(!app.should_show_modal);
    let _ = tui.exit();
}

#[test]
fn app_dispatch_url_confirmed_sets_urls_and_quits() {
    let mut app = App::new(AppMode::Selector).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    let urls = vec!["https://a.com".into(), "https://b.com".into()];
    app.dispatch_action(Action::UrlConfirmed(urls.clone()), &mut tui)
        .expect("dispatch");
    assert!(app.should_quit);
    match &app.result {
        AppResult::Urls(v) => assert_eq!(v, &urls),
        other => panic!("Expected Urls, got {other:?}"),
    }
    let _ = tui.exit();
}

#[test]
fn app_dispatch_url_cancelled_sets_empty_vec() {
    let mut app = App::new(AppMode::Selector).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    app.dispatch_action(Action::UrlCancelled, &mut tui)
        .expect("dispatch");
    assert!(app.should_quit);
    match &app.result {
        AppResult::Urls(v) => assert!(v.is_empty()),
        other => panic!("Expected empty Urls, got {other:?}"),
    }
    let _ = tui.exit();
}

#[test]
fn app_dispatch_config_done_sets_value() {
    let mut app = App::new(AppMode::Config).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    let value = serde_json::json!({"key": "value"});
    app.dispatch_action(Action::ConfigDone(Some(value.clone())), &mut tui)
        .expect("dispatch");
    assert!(app.should_quit);
    match &app.result {
        AppResult::Config(Some(v)) => assert_eq!(v, &value),
        other => panic!("Expected Config(Some), got {other:?}"),
    }
    let _ = tui.exit();
}

#[test]
fn app_dispatch_config_cancelled_sets_none() {
    let mut app = App::new(AppMode::Config).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    app.dispatch_action(Action::ConfigCancelled, &mut tui)
        .expect("dispatch");
    assert!(app.should_quit);
    assert!(matches!(app.result, AppResult::Config(None)));
    let _ = tui.exit();
}

#[test]
fn app_dispatch_tick_does_not_quit() {
    let mut app = App::new(AppMode::Selector).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    app.dispatch_action(Action::Tick, &mut tui)
        .expect("dispatch");
    assert!(!app.should_quit);
    let _ = tui.exit();
}

#[test]
fn app_dispatch_render_does_not_quit() {
    let mut app = App::new(AppMode::Selector).expect("App::new");
    let mut tui = Tui::new().expect("Tui::new");
    tui.enter().expect("tui.enter");
    app.dispatch_action(Action::Render, &mut tui)
        .expect("dispatch");
    assert!(!app.should_quit);
    let _ = tui.exit();
}

// ===========================================================================
// Group 3: HelpModal Escape Handling
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
        ("↑↓".into(), "Navigate".into()),
        ("Space".into(), "Toggle".into()),
        ("Enter".into(), "Confirm".into()),
        ("Esc".into(), "Close".into()),
    ];
    let modal = HelpModal::new("Keybindings".into(), bindings);
    assert_eq!(modal.bindings.len(), 4);
    assert_eq!(modal.title, "Keybindings");
}

// ===========================================================================
// Group 4: centered_rect Geometry
// ===========================================================================

#[test]
fn centered_rect_60x50_returns_centered_region() {
    let area = ratatui::layout::Rect::new(0, 0, 120, 40);
    let rect = centered_rect(60, 50, area);
    // Width ≈ 60% of 120 = 72
    assert!(
        rect.width >= 65 && rect.width <= 80,
        "width: {}",
        rect.width
    );
    // Height ≈ 50% of 40 = 20
    assert!(
        rect.height >= 15 && rect.height <= 25,
        "height: {}",
        rect.height
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

// ===========================================================================
// Group 5: ConfigForm Key Events
// ===========================================================================

#[test]
fn config_form_q_cancels() {
    let mut form = ConfigFormState::new_default();
    let action = form.handle_key_event(key(KeyCode::Char('q')));
    assert!(form.cancelled);
    assert!(form.is_done());
    assert!(matches!(action, Some(Action::ConfigCancelled)));
}

#[test]
fn config_form_uppercase_q_cancels() {
    let mut form = ConfigFormState::new_default();
    let action = form.handle_key_event(key(KeyCode::Char('Q')));
    assert!(form.cancelled);
    assert!(matches!(action, Some(Action::ConfigCancelled)));
}

#[test]
fn config_form_question_mark_toggles_help() {
    let mut form = ConfigFormState::new_default();
    let action = form.handle_key_event(key(KeyCode::Char('?')));
    assert!(matches!(action, Some(Action::ToggleHelp)));
}

#[test]
fn config_form_ctrl_s_submits() {
    let mut form = ConfigFormState::new_default();
    let action = form.handle_key_event(key_ctrl(KeyCode::Char('s')));
    assert!(form.submitted);
    assert!(form.is_done());
    assert!(matches!(action, Some(Action::ConfigDone(_))));
}

#[test]
fn config_form_data_is_json_object() {
    let form = ConfigFormState::new_default();
    let data = form.data();
    assert!(data.is_object());
}

#[test]
fn config_form_navigation_keys_do_not_crash() {
    let mut form = ConfigFormState::new_default();
    // Navigate through fields
    for _ in 0..20 {
        form.handle_input(key(KeyCode::Down));
    }
    for _ in 0..20 {
        form.handle_input(key(KeyCode::Up));
    }
    form.handle_input(key(KeyCode::Enter));
    form.handle_input(key(KeyCode::Esc));
    form.handle_input(key(KeyCode::Tab));
    form.handle_input(key(KeyCode::BackTab));
    // Should still be active (not submitted or cancelled)
    assert!(!form.is_done());
}

#[test]
fn config_form_mark_methods() {
    let mut form = ConfigFormState::new_default();
    form.mark_submitted();
    assert!(form.submitted);
    assert!(form.is_done());

    let mut form2 = ConfigFormState::new_default();
    form2.mark_cancelled();
    assert!(form2.cancelled);
    assert!(form2.is_done());
}
