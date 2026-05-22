//! Catppuccin Mocha theme for TUI.
//!
//! Semantic color mapping for consistent terminal rendering.
//! Uses the catppuccin crate palette values converted to ratatui colors.

use ratatui::style::Color;

/// Semantic color roles mapped to Catppuccin Mocha palette.
pub struct Theme;

impl Theme {
    // Status colors
    pub fn error() -> Color {
        Color::Rgb(243, 139, 168)
    }
    pub fn warning() -> Color {
        Color::Rgb(249, 226, 175)
    }
    pub fn success() -> Color {
        Color::Rgb(166, 227, 161)
    }
    pub fn processing() -> Color {
        Color::Rgb(137, 180, 250)
    }

    // Text hierarchy
    pub fn text() -> Color {
        Color::Rgb(205, 214, 244)
    }
    pub fn text_muted() -> Color {
        Color::Rgb(147, 153, 178)
    }
    pub fn text_subtle() -> Color {
        Color::Rgb(127, 132, 156)
    }

    // Accent
    pub fn accent() -> Color {
        Color::Rgb(148, 226, 213)
    }
    pub fn highlight() -> Color {
        Color::Rgb(249, 226, 175)
    }

    // Special
    pub fn parse_error() -> Color {
        Color::Rgb(203, 166, 247)
    }
    pub fn background() -> Color {
        Color::Rgb(30, 30, 46)
    }
    pub fn surface() -> Color {
        Color::Rgb(49, 50, 68)
    }
}
