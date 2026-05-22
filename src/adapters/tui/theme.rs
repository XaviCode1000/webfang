//! Theme colours for the TUI — Catppuccin Mocha palette.
//!
//! Provides a central Theme struct with static methods so every widget
//! gets consistent colours without repeating hex values.

use ratatui::style::Color;

/// Catppuccin Mocha colour tokens for the terminal UI.
pub struct Theme;

impl Theme {
    /// Primary accent — Blue `#89b4fa`
    pub fn accent() -> Color {
        Color::Rgb(0x89, 0xb4, 0xfa)
    }

    /// Primary text — Text `#cdd6f4`
    pub fn text() -> Color {
        Color::Rgb(0xcd, 0xd6, 0xf4)
    }

    /// Subtle text (labels, separators) — Subtext0 `#a6adc8`
    pub fn text_subtle() -> Color {
        Color::Rgb(0xa6, 0xad, 0xc8)
    }

    /// Muted text (status, hints) — Overlay0 `#6c7086`
    pub fn text_muted() -> Color {
        Color::Rgb(0x6c, 0x70, 0x86)
    }

    /// Warning / attention — Yellow `#f9e2af`
    pub fn warning() -> Color {
        Color::Rgb(0xf9, 0xe2, 0xaf)
    }

    /// Surface / border colour — Surface0 `#313244`
    pub fn surface() -> Color {
        Color::Rgb(0x31, 0x32, 0x44)
    }

    /// Success / completed — Green `#a6e3a1`
    pub fn success() -> Color {
        Color::Rgb(0xa6, 0xe3, 0xa1)
    }

    /// Error / failure — Red `#f38ba8`
    pub fn error() -> Color {
        Color::Rgb(0xf3, 0x8b, 0xa8)
    }

    /// Background / base colour — Base `#1e1e2e`
    pub fn background() -> Color {
        Color::Rgb(0x1e, 0x1e, 0x2e)
    }

    /// Processing / active state — Sky `#89dceb`
    pub fn processing() -> Color {
        Color::Rgb(0x89, 0xdc, 0xeb)
    }

    /// Highlight / cursor — Lavender `#b4befe`
    pub fn highlight() -> Color {
        Color::Rgb(0xb4, 0xbe, 0xfe)
    }

    /// Parse error / warning — Peach `#fab387`
    pub fn parse_error() -> Color {
        Color::Rgb(0xfa, 0xb3, 0x87)
    }
}
