//! Integration tests for TUI unification (Issue #137).
//!
//! Tests CollapsibleConfig, Theme colors, Preflight config application,
//! and Args parsing — all without terminal rendering or network calls.
//!
//! Only exercises public API: `Component::handle_key_event`, `to_json()`,
//! `url()`, public fields, and free functions.

// TUI integration tests require the `ui` feature (ratatui + crossterm).
#![cfg(feature = "ui")]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rust_scraper::adapters::tui::collapsible_config::CollapsibleConfig;
use rust_scraper::adapters::tui::component::Component;
use rust_scraper::adapters::tui::theme::{Theme, ThemeMode};
use rust_scraper::cli::preflight::apply_tui_config_args;
use rust_scraper::domain::JsStrategy;
use rust_scraper::{Args, ExportFormat, OutputFormat, Parser};
use std::path::PathBuf;

// ============================================================================
// Helpers
// ============================================================================

fn default_args() -> Args {
    Args {
        url: Some("https://example.com".into()),
        ..Default::default()
    }
}

fn config_json(pairs: &[(&str, serde_json::Value)]) -> serde_json::Value {
    serde_json::Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn key_ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

// ============================================================================
// 1. CollapsibleConfig — keyboard navigation via Component trait
// ============================================================================

#[test]
fn collapsible_config_creates_with_eight_sections() {
    let config = CollapsibleConfig::new();
    let json = config.to_json();
    let obj = json.as_object().unwrap();
    // Should have merged keys from all 8 sections
    assert!(obj.len() >= 8);
}

#[test]
fn collapsible_config_initial_not_submitted_or_cancelled() {
    let config = CollapsibleConfig::new();
    assert!(!config.submitted);
    assert!(!config.cancelled);
}

#[test]
fn collapsible_config_q_cancels() {
    let mut config = CollapsibleConfig::new();
    let action = config.handle_key_event(key(KeyCode::Char('q'))).unwrap();
    assert!(config.cancelled);
    assert!(matches!(
        action,
        Some(rust_scraper::adapters::tui::action::Action::ConfigCancelled)
    ));
}

#[test]
fn collapsible_config_uppercase_q_cancels() {
    let mut config = CollapsibleConfig::new();
    let action = config.handle_key_event(key(KeyCode::Char('Q'))).unwrap();
    assert!(config.cancelled);
    assert!(matches!(
        action,
        Some(rust_scraper::adapters::tui::action::Action::ConfigCancelled)
    ));
}

#[test]
fn collapsible_config_ctrl_s_submits() {
    let mut config = CollapsibleConfig::new();
    let action = config
        .handle_key_event(key_ctrl(KeyCode::Char('s')))
        .unwrap();
    assert!(config.submitted);
    assert!(matches!(
        action,
        Some(rust_scraper::adapters::tui::action::Action::ConfigDone(_))
    ));
}

#[test]
fn collapsible_config_question_mark_toggles_help() {
    let mut config = CollapsibleConfig::new();
    let action = config.handle_key_event(key(KeyCode::Char('?'))).unwrap();
    assert!(matches!(
        action,
        Some(rust_scraper::adapters::tui::action::Action::ToggleHelp)
    ));
}

#[test]
fn collapsible_config_down_key_navigates() {
    let mut config = CollapsibleConfig::new();
    assert_eq!(config.focused_section_index(), 0);
    // Move down twice — cursor should advance
    let _ = config.handle_key_event(key(KeyCode::Down));
    let _ = config.handle_key_event(key(KeyCode::Down));
    assert_eq!(config.focused_section_index(), 2);
}

#[test]
fn collapsible_config_up_key_navigates() {
    let mut config = CollapsibleConfig::new();
    // Move down then up — should be back at start
    let _ = config.handle_key_event(key(KeyCode::Down));
    let _ = config.handle_key_event(key(KeyCode::Up));
    assert_eq!(config.focused_section_index(), 0);
}

#[test]
fn collapsible_config_enter_expands_section() {
    let mut config = CollapsibleConfig::new();
    // Target (index 0) is already expanded by default
    assert!(config.is_section_expanded(0));
    // Enter on an expanded section enters field-edit mode
    let _ = config.handle_key_event(key(KeyCode::Enter));
    assert_eq!(config.focused_section_index(), 0);
    // Esc returns to section list
    let _ = config.handle_key_event(key(KeyCode::Esc));
    assert_eq!(config.focused_section_index(), 0);
}

#[test]
fn collapsible_config_space_toggles() {
    let mut config = CollapsibleConfig::new();
    // Target (index 0) starts expanded
    assert!(config.is_section_expanded(0));
    // Space collapses it
    let _ = config.handle_key_event(key(KeyCode::Char(' ')));
    assert!(!config.is_section_expanded(0));
    // Space expands it again
    let _ = config.handle_key_event(key(KeyCode::Char(' ')));
    assert!(config.is_section_expanded(0));
}

#[test]
fn collapsible_config_url_extracts_from_target_section() {
    let config = CollapsibleConfig::new();
    let url = config.url();
    // Default form has empty URL field — should return None or empty
    assert!(url.is_none() || url.as_deref() == Some(""));
}

#[test]
fn collapsible_config_navigation_does_not_panic() {
    let mut config = CollapsibleConfig::new();
    // Navigate through all sections and beyond
    for _ in 0..20 {
        let _ = config.handle_key_event(key(KeyCode::Down));
    }
    for _ in 0..20 {
        let _ = config.handle_key_event(key(KeyCode::Up));
    }
    assert!(!config.submitted);
    assert!(!config.cancelled);
    assert!(config.focused_section_index() < 8);
}

#[test]
fn collapsible_config_ctrl_s_returns_config_done_with_json() {
    let mut config = CollapsibleConfig::new();
    let action = config
        .handle_key_event(key_ctrl(KeyCode::Char('s')))
        .unwrap();
    match action {
        Some(rust_scraper::adapters::tui::action::Action::ConfigDone(Some(value))) => {
            assert!(value.is_object());
            let obj = value.as_object().unwrap();
            assert!(obj.contains_key("url"));
        },
        _ => panic!("Expected ConfigDone with Some(value)"),
    }
}

#[test]
fn collapsible_config_esc_in_section_list_mode_is_noop() {
    let mut config = CollapsibleConfig::new();
    // In SectionList mode, Esc doesn't do anything special (not in field edit)
    let _ = config.handle_key_event(key(KeyCode::Esc));
    assert!(!config.submitted);
    assert!(!config.cancelled);
}

#[test]
fn collapsible_config_enter_then_esc_returns_to_section_list() {
    let mut config = CollapsibleConfig::new();
    // Enter field edit mode on Target
    let _ = config.handle_key_event(key(KeyCode::Enter));
    // Esc back to section list
    let _ = config.handle_key_event(key(KeyCode::Esc));
    // Down should work now (back in section list)
    let _ = config.handle_key_event(key(KeyCode::Down));
    let json = config.to_json();
    assert!(json.is_object());
}

#[test]
fn collapsible_config_navigate_to_output_toggle_expand() {
    let mut config = CollapsibleConfig::new();
    // Move to Output (index 1, collapsed by default)
    let _ = config.handle_key_event(key(KeyCode::Down));
    // Space should expand it
    let _ = config.handle_key_event(key(KeyCode::Char(' ')));
    // Enter should enter field edit
    let _ = config.handle_key_event(key(KeyCode::Enter));
    // Esc back
    let _ = config.handle_key_event(key(KeyCode::Esc));
    // Space should collapse it
    let _ = config.handle_key_event(key(KeyCode::Char(' ')));
}

// ============================================================================
// 2. Theme Color Operations
// ============================================================================

// --- Color values are valid Rgb ---

#[test]
fn theme_all_named_colors_are_rgb() {
    let colors = [
        Theme::accent(),
        Theme::text(),
        Theme::text_subtle(),
        Theme::text_muted(),
        Theme::warning(),
        Theme::surface(),
        Theme::success(),
        Theme::error(),
        Theme::background(),
        Theme::processing(),
        Theme::highlight(),
        Theme::parse_error(),
    ];
    for c in &colors {
        match c {
            ratatui::style::Color::Rgb(_, _, _) => {},
            _ => panic!("All theme colors must be Rgb variants"),
        }
    }
}

// --- All colors are distinct ---

#[test]
fn theme_base_colors_are_distinct() {
    let colors = [
        ("accent", Theme::accent()),
        ("text", Theme::text()),
        ("text_subtle", Theme::text_subtle()),
        ("text_muted", Theme::text_muted()),
        ("warning", Theme::warning()),
        ("surface", Theme::surface()),
        ("success", Theme::success()),
        ("error", Theme::error()),
        ("background", Theme::background()),
        ("processing", Theme::processing()),
        ("highlight", Theme::highlight()),
        ("parse_error", Theme::parse_error()),
    ];
    for (i, (name_a, a)) in colors.iter().enumerate() {
        for (j, (name_b, b)) in colors.iter().enumerate() {
            if i != j {
                assert_ne!(a, b, "{name_a} and {name_b} should be distinct");
            }
        }
    }
}

// --- Lighten / Darken ---

#[test]
fn theme_lighten_produces_different_color() {
    let dark = ratatui::style::Color::Rgb(0x1e, 0x1e, 0x2e);
    let light = Theme::lighten(dark, 0.3);
    assert_ne!(dark, light);
}

#[test]
fn theme_darken_produces_different_color() {
    let light = ratatui::style::Color::Rgb(0xcd, 0xd6, 0xf4);
    let dark = Theme::darken(light, 0.3);
    assert_ne!(light, dark);
}

// --- Non-Rgb colors must not panic (Issue #152) ---

#[test]
fn theme_lighten_does_not_panic_on_non_rgb() {
    let reset = ratatui::style::Color::Reset;
    let black = ratatui::style::Color::Black;
    let dark_gray = ratatui::style::Color::DarkGray;
    // Non-Rgb colors are returned unchanged (identity), never panicked.
    assert_eq!(Theme::lighten(reset, 0.3), reset);
    assert_eq!(Theme::lighten(black, 0.3), black);
    assert_eq!(Theme::lighten(dark_gray, 0.3), dark_gray);
}

#[test]
fn theme_darken_does_not_panic_on_non_rgb() {
    let reset = ratatui::style::Color::Reset;
    let black = ratatui::style::Color::Black;
    // Non-Rgb colors are returned unchanged (identity), never panicked.
    assert_eq!(Theme::darken(reset, 0.3), reset);
    assert_eq!(Theme::darken(black, 0.3), black);
}

#[test]
fn theme_lighten_channel_values_increase() {
    let dark = ratatui::style::Color::Rgb(0x1e, 0x1e, 0x2e);
    let light = Theme::lighten(dark, 0.3);
    if let (ratatui::style::Color::Rgb(lr, lg, lb), ratatui::style::Color::Rgb(dr, dg, db)) =
        (light, dark)
    {
        assert!(
            lr > dr || lg > dg || lb > db,
            "lightened color should be brighter"
        );
    }
}

#[test]
fn theme_darken_channel_values_decrease() {
    let light = ratatui::style::Color::Rgb(0xcd, 0xd6, 0xf4);
    let dark = Theme::darken(light, 0.3);
    if let (ratatui::style::Color::Rgb(lr, lg, lb), ratatui::style::Color::Rgb(dr, dg, db)) =
        (light, dark)
    {
        assert!(
            dr < lr || dg < lg || db < lb,
            "darkened color should be darker"
        );
    }
}

// --- Shift Hue ---

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

// --- WCAG Contrast ---

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
fn theme_contrast_ratio_is_positive() {
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
fn theme_contrast_text_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::text(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_error_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::error(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_success_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::success(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_warning_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::warning(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_accent_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::accent(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_processing_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::processing(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_highlight_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::highlight(), Theme::background()) >= 4.5);
}

#[test]
fn theme_contrast_parse_error_meets_wcag_aa() {
    assert!(Theme::contrast(Theme::parse_error(), Theme::background()) >= 4.5);
}

// --- Adaptive Theme ---

#[test]
fn theme_adaptive_dark_returns_theme() {
    let _ = Theme::adaptive(ThemeMode::Dark);
}

#[test]
fn theme_adaptive_light_returns_theme() {
    let _ = Theme::adaptive(ThemeMode::Light);
}

#[test]
fn theme_adaptive_high_contrast_returns_theme() {
    let _ = Theme::adaptive(ThemeMode::HighContrast);
}

// ============================================================================
// 3. Preflight Config Application (apply_tui_config_args)
// ============================================================================

// --- Target fields ---

#[test]
fn apply_tui_config_sets_url() {
    let args = default_args();
    let json = config_json(&[("url", serde_json::json!("https://target.com"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.url.as_deref(), Some("https://target.com"));
}

#[test]
fn apply_tui_config_sets_selector() {
    let args = default_args();
    let json = config_json(&[("selector", serde_json::json!("article.main"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.selector, "article.main");
}

#[test]
fn apply_tui_config_empty_string_selector_ignored() {
    let mut args = default_args();
    args.selector = "body".into();
    let json = config_json(&[("selector", serde_json::json!(""))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.selector, "body");
}

// --- Output fields ---

#[test]
fn apply_tui_config_sets_output_dir() {
    let args = default_args();
    let json = config_json(&[("output", serde_json::json!("/custom/output"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.output, PathBuf::from("/custom/output"));
}

#[test]
fn apply_tui_config_sets_format_json() {
    let args = default_args();
    let json = config_json(&[("format", serde_json::json!("json"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.format, OutputFormat::Json);
}

#[test]
fn apply_tui_config_sets_format_text() {
    let args = default_args();
    let json = config_json(&[("format", serde_json::json!("text"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.format, OutputFormat::Text);
}

#[test]
fn apply_tui_config_sets_format_markdown() {
    let args = default_args();
    let json = config_json(&[("format", serde_json::json!("markdown"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.format, OutputFormat::Markdown);
}

#[test]
fn apply_tui_config_unknown_format_defaults_to_markdown() {
    let args = default_args();
    let json = config_json(&[("format", serde_json::json!("unknown"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.format, OutputFormat::Markdown);
}

#[test]
fn apply_tui_config_sets_export_format_vector() {
    let args = default_args();
    let json = config_json(&[("export_format", serde_json::json!("vector"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.export_format, ExportFormat::Vector);
}

#[test]
fn apply_tui_config_sets_export_format_auto() {
    let args = default_args();
    let json = config_json(&[("export_format", serde_json::json!("auto"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.export_format, ExportFormat::Auto);
}

#[test]
fn apply_tui_config_sets_export_format_jsonl() {
    let args = default_args();
    let json = config_json(&[("export_format", serde_json::json!("jsonl"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.export_format, ExportFormat::Jsonl);
}

// --- Discovery fields ---

#[test]
fn apply_tui_config_sets_use_sitemap() {
    let args = default_args();
    let json = config_json(&[("use_sitemap", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.use_sitemap);
}

#[test]
fn apply_tui_config_sets_max_pages() {
    let args = default_args();
    let json = config_json(&[("max_pages", serde_json::json!("50"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.max_pages, 50);
}

#[test]
fn apply_tui_config_sets_max_depth() {
    let args = default_args();
    let json = config_json(&[("max_depth", serde_json::json!("5"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.max_depth, 5);
}

#[test]
fn apply_tui_config_sets_sitemap_depth() {
    let args = default_args();
    let json = config_json(&[("sitemap_depth", serde_json::json!("7"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.sitemap_depth, 7);
}

#[test]
fn apply_tui_config_sets_sitemap_url() {
    let args = default_args();
    let json = config_json(&[(
        "sitemap_url",
        serde_json::json!("https://example.com/sitemap.xml"),
    )]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(
        result.sitemap_url.as_deref(),
        Some("https://example.com/sitemap.xml")
    );
}

// --- Crawler fields ---

#[test]
fn apply_tui_config_sets_timeout_secs() {
    let args = default_args();
    let json = config_json(&[("timeout_secs", serde_json::json!("60"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.timeout_secs, 60);
}

#[test]
fn apply_tui_config_sets_max_retries() {
    let args = default_args();
    let json = config_json(&[("max_retries", serde_json::json!("5"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.max_retries, 5);
}

#[test]
fn apply_tui_config_sets_delay_ms() {
    let args = default_args();
    let json = config_json(&[("delay_ms", serde_json::json!("2000"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.delay_ms, 2000);
}

#[test]
fn apply_tui_config_concurrency_auto() {
    let args = default_args();
    let json = config_json(&[("concurrency", serde_json::json!("auto"))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.concurrency.is_auto());
}

#[test]
fn apply_tui_config_concurrency_fixed_number() {
    let args = default_args();
    let json = config_json(&[("concurrency", serde_json::json!("8"))]);
    let result = apply_tui_config_args(args, &json);
    assert!(!result.concurrency.is_auto());
    assert_eq!(result.concurrency.get(), Some(8));
}

#[test]
fn apply_tui_config_sets_include_pattern() {
    let args = default_args();
    let json = config_json(&[("include_pattern", serde_json::json!("*/blog/*,*/docs/*"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.include_patterns, vec!["*/blog/*", "*/docs/*"]);
}

#[test]
fn apply_tui_config_sets_exclude_pattern() {
    let args = default_args();
    let json = config_json(&[("exclude_pattern", serde_json::json!("*/admin/*"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.exclude_patterns, vec!["*/admin/*"]);
}

// --- Network fields ---

#[test]
fn apply_tui_config_sets_user_agent() {
    let args = default_args();
    let json = config_json(&[("user_agent", serde_json::json!("CustomAgent/1.0"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.user_agent.as_deref(), Some("CustomAgent/1.0"));
}

#[test]
fn apply_tui_config_sets_accept_language() {
    let args = default_args();
    let json = config_json(&[("accept_language", serde_json::json!("es-ES,es;q=0.9"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.accept_language, "es-ES,es;q=0.9");
}

#[test]
fn apply_tui_config_sets_h2_profile() {
    let args = default_args();
    let json = config_json(&[("h2_profile", serde_json::json!("Chrome131"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.h2_profile, "Chrome131");
}

#[test]
fn apply_tui_config_sets_js_strategy_hybrid() {
    let args = default_args();
    let json = config_json(&[("js_strategy", serde_json::json!("hybrid"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.js_strategy, JsStrategy::Hybrid);
}

#[test]
fn apply_tui_config_sets_js_strategy_full() {
    let args = default_args();
    let json = config_json(&[("js_strategy", serde_json::json!("full"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.js_strategy, JsStrategy::Full);
}

#[test]
fn apply_tui_config_sets_js_strategy_static() {
    let args = default_args();
    let json = config_json(&[("js_strategy", serde_json::json!("static"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.js_strategy, JsStrategy::Static);
}

#[test]
fn apply_tui_config_sets_force_js_render() {
    let args = default_args();
    let json = config_json(&[("force_js_render", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.force_js_render);
}

// --- Download fields ---

#[test]
fn apply_tui_config_sets_download_images() {
    let args = default_args();
    let json = config_json(&[("download_images", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.download_images);
}

#[test]
fn apply_tui_config_sets_download_documents() {
    let args = default_args();
    let json = config_json(&[("download_documents", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.download_documents);
}

#[test]
fn apply_tui_config_sets_max_file_size() {
    let args = default_args();
    let json = config_json(&[("max_file_size", serde_json::json!("100000000"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.max_file_size, 100_000_000);
}

#[test]
fn apply_tui_config_sets_download_timeout() {
    let args = default_args();
    let json = config_json(&[("download_timeout", serde_json::json!("60"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.download_timeout, 60);
}

// --- Obsidian fields ---

#[test]
fn apply_tui_config_sets_obsidian_wiki_links() {
    let args = default_args();
    let json = config_json(&[("obsidian_wiki_links", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.obsidian_wiki_links);
}

#[test]
fn apply_tui_config_sets_obsidian_tags_comma_separated() {
    let args = default_args();
    let json = config_json(&[("obsidian_tags", serde_json::json!("scraping,ai,web"))]);
    let result = apply_tui_config_args(args, &json);
    let tags = result.obsidian_tags.unwrap();
    assert_eq!(tags, vec!["scraping", "ai", "web"]);
}

#[test]
fn apply_tui_config_sets_obsidian_tags_single() {
    let args = default_args();
    let json = config_json(&[("obsidian_tags", serde_json::json!("research"))]);
    let result = apply_tui_config_args(args, &json);
    let tags = result.obsidian_tags.unwrap();
    assert_eq!(tags, vec!["research"]);
}

#[test]
fn apply_tui_config_sets_obsidian_relative_assets() {
    let args = default_args();
    let json = config_json(&[("obsidian_relative_assets", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.obsidian_relative_assets);
}

#[test]
fn apply_tui_config_sets_obsidian_rich_metadata() {
    let args = default_args();
    let json = config_json(&[("obsidian_rich_metadata", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.obsidian_rich_metadata);
}

#[test]
fn apply_tui_config_sets_vault_path() {
    let args = default_args();
    let json = config_json(&[("vault", serde_json::json!("/home/user/MyVault"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.vault, Some(PathBuf::from("/home/user/MyVault")));
}

#[test]
fn apply_tui_config_sets_quick_save() {
    let args = default_args();
    let json = config_json(&[("quick_save", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.quick_save);
}

// --- Advanced fields ---

#[test]
fn apply_tui_config_sets_elastic() {
    let args = default_args();
    let json = config_json(&[("elastic", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.elastic);
}

#[test]
fn apply_tui_config_sets_pipeline() {
    let args = default_args();
    let json = config_json(&[("pipeline", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.pipeline);
}

#[test]
fn apply_tui_config_sets_batch() {
    let args = default_args();
    let json = config_json(&[("batch", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.batch);
}

#[test]
fn apply_tui_config_sets_checkpoint_interval() {
    let args = default_args();
    let json = config_json(&[("checkpoint_interval", serde_json::json!("50"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.checkpoint_interval, 50);
}

#[test]
fn apply_tui_config_sets_autoscale() {
    let args = default_args();
    let json = config_json(&[("autoscale", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.autoscale);
}

#[test]
fn apply_tui_config_sets_verbose() {
    let args = default_args();
    let json = config_json(&[("verbose", serde_json::json!("2"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.verbose, 2);
}

#[test]
fn apply_tui_config_sets_quiet() {
    let args = default_args();
    let json = config_json(&[("quiet", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.quiet);
}

#[test]
fn apply_tui_config_sets_dry_run() {
    let args = default_args();
    let json = config_json(&[("dry_run", serde_json::json!(true))]);
    let result = apply_tui_config_args(args, &json);
    assert!(result.dry_run);
}

// --- Edge cases ---

#[test]
fn apply_tui_config_empty_json_no_change() {
    let mut args = default_args();
    args.selector = "body".into();
    args.max_pages = 10;
    args.format = OutputFormat::Markdown;
    let json = serde_json::json!({});
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.selector, "body");
    assert_eq!(result.max_pages, 10);
    assert_eq!(result.format, OutputFormat::Markdown);
}

#[test]
fn apply_tui_config_null_values_ignored() {
    let mut args = default_args();
    args.selector = "body".into();
    args.max_pages = 10;
    let pairs = vec![
        ("selector", serde_json::json!(null)),
        ("max_pages", serde_json::json!(null)),
    ];
    let json = config_json(&pairs);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.selector, "body");
    assert_eq!(result.max_pages, 10);
}

#[test]
fn apply_tui_config_invalid_number_ignored() {
    let mut args = default_args();
    args.max_pages = 10;
    let json = config_json(&[("max_pages", serde_json::json!("not_a_number"))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.max_pages, 10);
}

#[test]
fn apply_tui_config_empty_string_path_ignored() {
    let mut args = default_args();
    args.output = PathBuf::from("output");
    let json = config_json(&[("output", serde_json::json!(""))]);
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.output, PathBuf::from("output"));
}

#[test]
fn apply_tui_config_all_fields_at_once() {
    let args = default_args();
    let json = config_json(&[
        ("url", serde_json::json!("https://full-test.com")),
        ("selector", serde_json::json!("#content")),
        ("output", serde_json::json!("/full/output")),
        ("format", serde_json::json!("json")),
        ("export_format", serde_json::json!("vector")),
        ("use_sitemap", serde_json::json!(true)),
        ("max_pages", serde_json::json!("100")),
        ("max_depth", serde_json::json!("4")),
        ("timeout_secs", serde_json::json!("45")),
        ("delay_ms", serde_json::json!("500")),
        ("concurrency", serde_json::json!("16")),
        ("download_images", serde_json::json!(true)),
        ("obsidian_wiki_links", serde_json::json!(true)),
        ("elastic", serde_json::json!(true)),
        ("dry_run", serde_json::json!(true)),
        ("js_strategy", serde_json::json!("hybrid")),
        ("obsidian_tags", serde_json::json!("a,b,c")),
    ]);
    let result = apply_tui_config_args(args, &json);

    assert_eq!(result.url.as_deref(), Some("https://full-test.com"));
    assert_eq!(result.selector, "#content");
    assert_eq!(result.output, PathBuf::from("/full/output"));
    assert_eq!(result.format, OutputFormat::Json);
    assert_eq!(result.export_format, ExportFormat::Vector);
    assert!(result.use_sitemap);
    assert_eq!(result.max_pages, 100);
    assert_eq!(result.max_depth, 4);
    assert_eq!(result.timeout_secs, 45);
    assert_eq!(result.delay_ms, 500);
    assert!(!result.concurrency.is_auto());
    assert_eq!(result.concurrency.get(), Some(16));
    assert!(result.download_images);
    assert!(result.obsidian_wiki_links);
    assert!(result.elastic);
    assert!(result.dry_run);
    assert_eq!(result.js_strategy, JsStrategy::Hybrid);
    assert_eq!(result.obsidian_tags.unwrap(), vec!["a", "b", "c"]);
}

// ============================================================================
// 4. Args Parsing Integration
// ============================================================================

#[test]
fn args_tui_flag_parsed() {
    let args = Args::try_parse_from(["rust_scraper", "--tui"]).expect("--tui must parse");
    assert!(args.tui);
}

#[test]
fn args_tui_flag_default_false() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert!(!args.tui);
}

#[test]
fn args_interactive_flag_hidden_but_parseable() {
    let args =
        Args::try_parse_from(["rust_scraper", "--interactive"]).expect("--interactive must parse");
    assert!(args.interactive);
}

#[test]
fn args_config_tui_flag_hidden_but_parseable() {
    let args =
        Args::try_parse_from(["rust_scraper", "--config-tui"]).expect("--config-tui must parse");
    assert!(args.config_tui);
}

#[test]
fn args_selector_default_is_body() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert_eq!(args.selector, "body");
}

#[test]
fn args_selector_custom() {
    let args = Args::try_parse_from(["rust_scraper", "--selector", "article"])
        .expect("--selector must parse");
    assert_eq!(args.selector, "article");
}

#[test]
fn args_format_markdown_default() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert_eq!(args.format, OutputFormat::Markdown);
}

#[test]
fn args_format_json() {
    let args =
        Args::try_parse_from(["rust_scraper", "--format", "json"]).expect("--format must parse");
    assert_eq!(args.format, OutputFormat::Json);
}

#[test]
fn args_export_format_jsonl_default() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert_eq!(args.export_format, ExportFormat::Jsonl);
}

#[test]
fn args_export_format_vector() {
    let args = Args::try_parse_from(["rust_scraper", "--export-format", "vector"])
        .expect("--export-format must parse");
    assert_eq!(args.export_format, ExportFormat::Vector);
}

#[test]
fn args_js_strategy_static_default() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert_eq!(args.js_strategy, JsStrategy::Static);
}

#[test]
fn args_js_strategy_hybrid() {
    let args = Args::try_parse_from(["rust_scraper", "--js-strategy", "hybrid"])
        .expect("--js-strategy must parse");
    assert_eq!(args.js_strategy, JsStrategy::Hybrid);
}

#[test]
fn args_max_pages_default() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert_eq!(args.max_pages, 10);
}

#[test]
fn args_max_pages_custom() {
    let args = Args::try_parse_from(["rust_scraper", "--max-pages", "50"])
        .expect("--max-pages must parse");
    assert_eq!(args.max_pages, 50);
}

#[test]
fn args_timeout_secs_default() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert_eq!(args.timeout_secs, 30);
}

#[test]
fn args_max_depth_default() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert_eq!(args.max_depth, 2);
}

#[test]
fn args_verbose_count() {
    let args = Args::try_parse_from(["rust_scraper", "-vv"]).expect("-vv must parse");
    assert_eq!(args.verbose, 2);
}

#[test]
fn args_quiet_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--quiet"]).expect("--quiet must parse");
    assert!(args.quiet);
}

#[test]
fn args_dry_run_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--dry-run"]).expect("--dry-run must parse");
    assert!(args.dry_run);
}

#[test]
fn args_obsidian_wiki_links_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--obsidian-wiki-links"])
        .expect("--obsidian-wiki-links must parse");
    assert!(args.obsidian_wiki_links);
}

#[test]
fn args_elastic_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--elastic"]).expect("--elastic must parse");
    assert!(args.elastic);
}

#[test]
fn args_pipeline_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--pipeline"]).expect("--pipeline must parse");
    assert!(args.pipeline);
}

#[test]
fn args_use_sitemap_flag() {
    let args =
        Args::try_parse_from(["rust_scraper", "--use-sitemap"]).expect("--use-sitemap must parse");
    assert!(args.use_sitemap);
}

#[test]
fn args_concurrency_default_auto() {
    let args = Args::try_parse_from(["rust_scraper"]).expect("minimal parse");
    assert!(args.concurrency.is_auto());
}

#[test]
fn args_concurrency_fixed() {
    let args = Args::try_parse_from(["rust_scraper", "--concurrency", "8"])
        .expect("--concurrency must parse");
    assert!(!args.concurrency.is_auto());
    assert_eq!(args.concurrency.get(), Some(8));
}

#[test]
fn args_download_images_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--download-images"])
        .expect("--download-images must parse");
    assert!(args.download_images);
}

#[test]
fn args_download_documents_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--download-documents"])
        .expect("--download-documents must parse");
    assert!(args.download_documents);
}

#[test]
fn args_quick_save_flag() {
    let args =
        Args::try_parse_from(["rust_scraper", "--quick-save"]).expect("--quick-save must parse");
    assert!(args.quick_save);
}

#[test]
fn args_autoscale_flag() {
    let args =
        Args::try_parse_from(["rust_scraper", "--autoscale"]).expect("--autoscale must parse");
    assert!(args.autoscale);
}

#[test]
fn args_obsidian_tags_comma_separated() {
    let args = Args::try_parse_from(["rust_scraper", "--obsidian-tags", "ai,scraping,web"])
        .expect("--obsidian-tags must parse");
    let tags = args.obsidian_tags.unwrap();
    assert_eq!(tags, vec!["ai", "scraping", "web"]);
}

#[test]
fn args_vault_path() {
    let args = Args::try_parse_from(["rust_scraper", "--vault", "/home/user/MyVault"])
        .expect("--vault must parse");
    assert_eq!(args.vault, Some(PathBuf::from("/home/user/MyVault")));
}

#[test]
fn args_include_patterns_comma_separated() {
    let args = Args::try_parse_from(["rust_scraper", "--include-pattern", "*/blog/*,*/docs/*"])
        .expect("--include-pattern must parse");
    assert_eq!(args.include_patterns, vec!["*/blog/*", "*/docs/*"]);
}

#[test]
fn args_exclude_patterns_comma_separated() {
    let args = Args::try_parse_from(["rust_scraper", "--exclude-pattern", "*/admin/*"])
        .expect("--exclude-pattern must parse");
    assert_eq!(args.exclude_patterns, vec!["*/admin/*"]);
}

#[test]
fn args_force_js_render_flag() {
    let args = Args::try_parse_from(["rust_scraper", "--force-js-render"])
        .expect("--force-js-render must parse");
    assert!(args.force_js_render);
}

// ============================================================================
// 5. End-to-End: CollapsibleConfig JSON → apply_tui_config_args
// ============================================================================

#[test]
fn e2e_tui_config_to_args_url_not_overridden_when_empty() {
    let config = CollapsibleConfig::new();
    let json = config.to_json();
    let args = default_args();
    let result = apply_tui_config_args(args, &json);
    // URL is empty in default config, so it should not override
    assert_eq!(result.url.as_deref(), Some("https://example.com"));
}

#[test]
fn e2e_tui_config_to_args_preserves_defaults() {
    let config = CollapsibleConfig::new();
    let json = config.to_json();
    let args = default_args();
    let result = apply_tui_config_args(args, &json);
    assert_eq!(result.selector, "body");
    assert_eq!(result.format, OutputFormat::Markdown);
    assert_eq!(result.export_format, ExportFormat::Jsonl);
    assert_eq!(result.max_pages, 10);
    assert_eq!(result.timeout_secs, 30);
    assert!(result.concurrency.is_auto());
}

#[test]
fn e2e_custom_tui_config_applies_to_args() {
    let config = CollapsibleConfig::new();
    let mut json = config.to_json();

    if let Some(obj) = json.as_object_mut() {
        obj.insert("selector".into(), serde_json::json!("h1.title"));
        obj.insert("max_pages".into(), serde_json::json!("25"));
        obj.insert("format".into(), serde_json::json!("json"));
        obj.insert("download_images".into(), serde_json::json!(true));
    }

    let args = default_args();
    let result = apply_tui_config_args(args, &json);

    assert_eq!(result.selector, "h1.title");
    assert_eq!(result.max_pages, 25);
    assert_eq!(result.format, OutputFormat::Json);
    assert!(result.download_images);
}
