//! Theme colours for the TUI — Catppuccin Mocha palette.
//!
//! Provides a central Theme struct with static methods so every widget
//! gets consistent colours without repeating hex values.
//!
//! Uses ratatui's `palette` feature for color science operations:
//! - WCAG contrast validation via `palette::contrast`
//! - Color space conversions via `palette::Srgb`
//! - Theme generation via `Lighten`/`Darken`/`ShiftHue` traits

use ratatui::palette::{Darken, FromColor, Lch, Lighten, Srgb};
use ratatui::style::Color;

// ============================================================================
// Color conversion helpers
// ============================================================================

/// Convert a ratatui Color to palette Srgb.
///
/// Returns `None` for any non-`Rgb` variant (e.g. `Reset`, `Black`,
/// `AnsiValue`) instead of panicking, so callers can fall back to the
/// original color.
fn color_to_srgb(c: Color) -> Option<Srgb> {
    match c {
        Color::Rgb(r, g, b) => Some(Srgb::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
        )),
        _ => None,
    }
}

/// Convert palette Srgb back to ratatui Color.
fn srgb_to_color(c: Srgb) -> Color {
    let (r, g, b) = c.into_components();
    Color::Rgb((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

/// Calculate relative luminance per WCAG 2.0.
///
/// Uses the standard WCAG formula with sRGB linearization.
fn relative_luminance(c: Color) -> f64 {
    let (r, g, b) = match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => return 0.0,
    };
    let [r, g, b] = [r, g, b].map(|c| {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Calculate contrast ratio between two colors per WCAG 2.0.
fn contrast_ratio(c1: Color, c2: Color) -> f64 {
    let l1 = relative_luminance(c1);
    let l2 = relative_luminance(c2);
    let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (lighter + 0.05) / (darker + 0.05)
}

// ============================================================================
// Theme modes
// ============================================================================

/// Theme mode for adaptive color generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    /// Dark mode (default Catppuccin Mocha)
    Dark,
    /// Light mode (brightened colors)
    Light,
    /// High contrast mode (WCAG AAA compliant)
    HighContrast,
}

// ============================================================================
// Theme struct
// ============================================================================

/// Catppuccin Mocha colour tokens for the terminal UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Primary accent — Blue `#89b4fa`.
    pub accent: Color,
    /// Primary text — Text `#cdd6f4`.
    pub text: Color,
    /// Subtle text (labels, separators) — Subtext0 `#a6adc8`.
    pub text_subtle: Color,
    /// Muted text (status, hints) — Overlay0 `#6c7086`.
    pub text_muted: Color,
    /// Warning / attention — Yellow `#f9e2af`.
    pub warning: Color,
    /// Surface / border colour — Surface1 `#45475a`.
    pub surface: Color,
    /// Success / completed — Green `#a6e3a1`.
    pub success: Color,
    /// Error / failure — Red `#f38ba8`.
    pub error: Color,
    /// Background / base colour — Base `#1e1e2e`.
    pub background: Color,
    /// Processing / active state — Sky `#89dceb`.
    pub processing: Color,
    /// Highlight / cursor — Lavender `#b4befe`.
    pub highlight: Color,
    /// Parse error / warning — Peach `#fab387`.
    pub parse_error: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new_dark()
    }
}

impl Theme {
    /// Create the default dark theme (Catppuccin Mocha).
    pub fn new_dark() -> Self {
        Self {
            accent: Color::Rgb(0x89, 0xb4, 0xfa),
            text: Color::Rgb(0xcd, 0xd6, 0xf4),
            text_subtle: Color::Rgb(0xa6, 0xad, 0xc8),
            text_muted: Color::Rgb(0x6c, 0x70, 0x86),
            warning: Color::Rgb(0xf9, 0xe2, 0xaf),
            surface: Color::Rgb(0x45, 0x47, 0x5a),
            success: Color::Rgb(0xa6, 0xe3, 0xa1),
            error: Color::Rgb(0xf3, 0x8b, 0xa8),
            background: Color::Rgb(0x1e, 0x1e, 0x2e),
            processing: Color::Rgb(0x89, 0xdc, 0xeb),
            highlight: Color::Rgb(0xb4, 0xbe, 0xfe),
            parse_error: Color::Rgb(0xfa, 0xb3, 0x87),
        }
    }

    /// Lighten a color in perceptual (LCh) space by `amount` (-1.0..1.0).
    ///
    /// Positive `amount` brightens toward white, negative darkens toward black.
    /// Operates in LCh space for perceptually uniform results.
    fn lighten_perceptual(color: Color, amount: f32) -> Color {
        let Some(rgb) = color_to_srgb(color) else {
            return color;
        };
        let mut lch: Lch = Lch::from_color(rgb.into_linear());
        lch.l = (lch.l + amount * 100.0).clamp(0.0, 100.0);
        let res_rgb: Srgb<f32> = Srgb::from_color(lch);
        let (r, g, b) = res_rgb.into_components();
        Color::Rgb(
            (r * 255.0).round() as u8,
            (g * 255.0).round() as u8,
            (b * 255.0).round() as u8,
        )
    }

    /// Generate a theme variant based on mode.
    pub fn adaptive(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::new_dark(),
            ThemeMode::Light => Self::new_light(),
            ThemeMode::HighContrast => Self::new_high_contrast(),
        }
    }

    /// Create a light theme by perceptually brightening dark theme colors.
    fn new_light() -> Self {
        let d = Self::new_dark();
        Self {
            background: Self::lighten_perceptual(d.background, 0.45),
            surface: Self::lighten_perceptual(d.surface, 0.30),
            text: Self::lighten_perceptual(d.text, -0.15),
            text_subtle: Self::lighten_perceptual(d.text_subtle, -0.10),
            text_muted: Self::lighten_perceptual(d.text_muted, -0.05),
            accent: Self::lighten_perceptual(d.accent, -0.05),
            warning: Self::lighten_perceptual(d.warning, 0.10),
            success: Self::lighten_perceptual(d.success, 0.05),
            error: Self::lighten_perceptual(d.error, 0.05),
            processing: Self::lighten_perceptual(d.processing, 0.05),
            highlight: Self::lighten_perceptual(d.highlight, 0.05),
            parse_error: Self::lighten_perceptual(d.parse_error, 0.10),
        }
    }

    /// Create a high-contrast theme (WCAG AAA).
    fn new_high_contrast() -> Self {
        Self {
            background: Color::Rgb(0x00, 0x00, 0x00),
            surface: Color::Rgb(0x1a, 0x1a, 0x1a),
            text: Color::Rgb(0xff, 0xff, 0xff),
            text_subtle: Color::Rgb(0xcc, 0xcc, 0xcc),
            text_muted: Color::Rgb(0x99, 0x99, 0x99),
            accent: Color::Rgb(0xff, 0xff, 0x00),
            warning: Color::Rgb(0xff, 0xff, 0x00),
            success: Color::Rgb(0x00, 0xff, 0x00),
            error: Color::Rgb(0xff, 0x00, 0x00),
            processing: Color::Rgb(0x00, 0xff, 0xff),
            highlight: Color::Rgb(0xff, 0x00, 0xff),
            parse_error: Color::Rgb(0xff, 0x88, 0x00),
        }
    }

    // ------------------------------------------------------------------
    // Static compatibility methods (delegate to default dark theme)
    // ------------------------------------------------------------------

    /// Primary accent — Blue `#89b4fa`
    pub fn accent() -> Color {
        Self::default().accent
    }

    /// Primary text — Text `#cdd6f4`
    pub fn text() -> Color {
        Self::default().text
    }

    /// Subtle text (labels, separators) — Subtext0 `#a6adc8`
    pub fn text_subtle() -> Color {
        Self::default().text_subtle
    }

    /// Muted text (status, hints) — Overlay0 `#6c7086`
    pub fn text_muted() -> Color {
        Self::default().text_muted
    }

    /// Warning / attention — Yellow `#f9e2af`
    pub fn warning() -> Color {
        Self::default().warning
    }

    /// Surface / border colour — Surface1 `#45475a`
    pub fn surface() -> Color {
        Self::default().surface
    }

    /// Success / completed — Green `#a6e3a1`
    pub fn success() -> Color {
        Self::default().success
    }

    /// Error / failure — Red `#f38ba8`
    pub fn error() -> Color {
        Self::default().error
    }

    /// Background / base colour — Base `#1e1e2e`
    pub fn background() -> Color {
        Self::default().background
    }

    /// Processing / active state — Sky `#89dceb`
    pub fn processing() -> Color {
        Self::default().processing
    }

    /// Highlight / cursor — Lavender `#b4befe`
    pub fn highlight() -> Color {
        Self::default().highlight
    }

    /// Parse error / warning — Peach `#fab387`
    pub fn parse_error() -> Color {
        Self::default().parse_error
    }

    // ------------------------------------------------------------------
    // Color operations (using palette traits)
    // ------------------------------------------------------------------

    /// Lighten a color by a factor (0.0-1.0).
    ///
    /// Uses palette's `Lighten` trait for perceptually correct lightening.
    pub fn lighten(color: Color, factor: f32) -> Color {
        let Some(srgb) = color_to_srgb(color) else {
            return color;
        };
        let lighter: Srgb = srgb.lighten(factor);
        srgb_to_color(lighter)
    }

    /// Darken a color by a factor (0.0-1.0).
    ///
    /// Uses palette's `Darken` trait for perceptually correct darkening.
    pub fn darken(color: Color, factor: f32) -> Color {
        let Some(srgb) = color_to_srgb(color) else {
            return color;
        };
        let darker: Srgb = srgb.darken(factor);
        srgb_to_color(darker)
    }

    /// Shift hue of a color by degrees.
    ///
    /// Converts to HSL, shifts hue, converts back.
    pub fn shift_hue(color: Color, degrees: f32) -> Color {
        let (r, g, b) = match color {
            Color::Rgb(r, g, b) => (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
            _ => return color,
        };

        // RGB to HSL
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;

        if (max - min).abs() < 0.001 {
            return color; // Achromatic
        }

        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };

        let h = if max == r {
            ((g - b) / d + if g < b { 6.0 } else { 0.0 }) * 60.0
        } else if max == g {
            ((b - r) / d + 2.0) * 60.0
        } else {
            ((r - g) / d + 4.0) * 60.0
        };

        // Shift hue
        let new_h = (h + degrees) % 360.0;
        let new_h = if new_h < 0.0 { new_h + 360.0 } else { new_h };

        // HSL to RGB
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - ((new_h / 60.0) % 2.0 - 1.0).abs());
        let m = l - c / 2.0;

        let (r1, g1, b1) = if new_h < 60.0 {
            (c, x, 0.0)
        } else if new_h < 120.0 {
            (x, c, 0.0)
        } else if new_h < 180.0 {
            (0.0, c, x)
        } else if new_h < 240.0 {
            (0.0, x, c)
        } else if new_h < 300.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };

        Color::Rgb(
            ((r1 + m) * 255.0) as u8,
            ((g1 + m) * 255.0) as u8,
            ((b1 + m) * 255.0) as u8,
        )
    }

    // ------------------------------------------------------------------
    // WCAG contrast validation
    // ------------------------------------------------------------------

    /// Check if a foreground color has sufficient contrast against background.
    ///
    /// Returns true if contrast ratio >= threshold (default 4.5 for WCAG AA).
    pub fn has_contrast(fg: Color, bg: Color, threshold: f64) -> bool {
        contrast_ratio(fg, bg) >= threshold
    }

    /// Get contrast ratio between two colors.
    pub fn contrast(fg: Color, bg: Color) -> f64 {
        contrast_ratio(fg, bg)
    }

    // ------------------------------------------------------------------
    // Section colors (for collapsible config form)
    // ------------------------------------------------------------------

    /// Color for a section header based on expand state.
    ///
    /// Expanded sections get brighter accent, collapsed get muted.
    pub fn section_header(expanded: bool) -> Color {
        if expanded {
            Self::accent()
        } else {
            Self::text_subtle()
        }
    }

    /// Color for section content based on expand state.
    pub fn section_content(expanded: bool) -> Color {
        if expanded {
            Self::text()
        } else {
            Self::text_muted()
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // WCAG contrast tests (using palette-based helpers)
    // ------------------------------------------------------------------

    #[test]
    fn text_has_sufficient_contrast_against_background() {
        let ratio = contrast_ratio(Theme::text(), Theme::background());
        assert!(
            ratio >= 4.5,
            "text vs background contrast {ratio:.2} < 4.5 (WCAG AA)"
        );
    }

    #[test]
    fn error_has_sufficient_contrast() {
        let ratio = contrast_ratio(Theme::error(), Theme::background());
        assert!(
            ratio >= 4.5,
            "error vs background contrast {ratio:.2} < 4.5"
        );
    }

    #[test]
    fn success_has_sufficient_contrast() {
        let ratio = contrast_ratio(Theme::success(), Theme::background());
        assert!(
            ratio >= 4.5,
            "success vs background contrast {ratio:.2} < 4.5"
        );
    }

    #[test]
    fn warning_has_sufficient_contrast() {
        let ratio = contrast_ratio(Theme::warning(), Theme::background());
        assert!(
            ratio >= 4.5,
            "warning vs background contrast {ratio:.2} < 4.5"
        );
    }

    #[test]
    fn accent_has_sufficient_contrast() {
        let ratio = contrast_ratio(Theme::accent(), Theme::background());
        assert!(
            ratio >= 4.5,
            "accent vs background contrast {ratio:.2} < 4.5"
        );
    }

    #[test]
    fn text_muted_has_minimum_contrast() {
        let ratio = contrast_ratio(Theme::text_muted(), Theme::background());
        assert!(
            ratio >= 3.0,
            "text_muted vs background contrast {ratio:.2} < 3.0 (WCAG AA large text)"
        );
    }

    #[test]
    fn surface_has_distinct_border_contrast() {
        let ratio = contrast_ratio(Theme::surface(), Theme::background());
        assert!(
            ratio >= 1.5,
            "surface vs background contrast {ratio:.2} < 1.5 (minimum visibility)"
        );
    }

    #[test]
    fn processing_has_sufficient_contrast() {
        let ratio = contrast_ratio(Theme::processing(), Theme::background());
        assert!(
            ratio >= 4.5,
            "processing vs background contrast {ratio:.2} < 4.5"
        );
    }

    #[test]
    fn highlight_has_sufficient_contrast() {
        let ratio = contrast_ratio(Theme::highlight(), Theme::background());
        assert!(
            ratio >= 4.5,
            "highlight vs background contrast {ratio:.2} < 4.5"
        );
    }

    #[test]
    fn parse_error_has_sufficient_contrast() {
        let ratio = contrast_ratio(Theme::parse_error(), Theme::background());
        assert!(
            ratio >= 4.5,
            "parse_error vs background contrast {ratio:.2} < 4.5"
        );
    }

    #[test]
    fn text_subtle_has_minimum_contrast() {
        let ratio = contrast_ratio(Theme::text_subtle(), Theme::background());
        assert!(
            ratio >= 3.0,
            "text_subtle vs background contrast {ratio:.2} < 3.0 (WCAG AA large text)"
        );
    }

    // ------------------------------------------------------------------
    // Color conversion tests
    // ------------------------------------------------------------------

    #[test]
    fn color_to_srgb_roundtrip() {
        let original = Color::Rgb(0x89, 0xb4, 0xfa);
        let srgb = color_to_srgb(original).expect("Rgb color converts");
        let back = srgb_to_color(srgb);
        assert_eq!(original, back);
    }

    #[test]
    fn color_to_srgb_non_rgb_is_none() {
        assert!(color_to_srgb(Color::Reset).is_none());
        assert!(color_to_srgb(Color::Black).is_none());
    }

    #[test]
    fn lighten_increases_luminance() {
        let dark = Color::Rgb(0x1e, 0x1e, 0x2e);
        let light = Theme::lighten(dark, 0.3);
        let dark_lum = relative_luminance(dark);
        let light_lum = relative_luminance(light);
        assert!(light_lum > dark_lum, "lightened color should be brighter");
    }

    #[test]
    fn darken_decreases_luminance() {
        let light = Color::Rgb(0xcd, 0xd6, 0xf4);
        let dark = Theme::darken(light, 0.3);
        let light_lum = relative_luminance(light);
        let dark_lum = relative_luminance(dark);
        assert!(dark_lum < light_lum, "darkened color should be darker");
    }

    #[test]
    fn section_header_expanded_is_accent() {
        assert_eq!(Theme::section_header(true), Theme::accent());
    }

    #[test]
    fn section_header_collapsed_is_subtle() {
        assert_eq!(Theme::section_header(false), Theme::text_subtle());
    }

    #[test]
    fn has_contrast_returns_true_for_good_contrast() {
        assert!(Theme::has_contrast(Theme::text(), Theme::background(), 4.5));
    }

    #[test]
    fn has_contrast_returns_false_for_poor_contrast() {
        assert!(!Theme::has_contrast(
            Theme::surface(),
            Theme::background(),
            4.5
        ));
    }
}
