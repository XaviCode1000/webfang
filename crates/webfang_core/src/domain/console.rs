//! Console-presentation decisions shared by the CLI and infrastructure
//! logging layers (XP-K-04, #1608).
//!
//! Pure functions only: the streams themselves are touched by the callers,
//! so every decision here is unit-testable on every platform.

/// Whether ANSI styling should be emitted on the console stream.
///
/// ANSI escapes are only useful on an interactive terminal. When the stream
/// is redirected — a file, a pipe, CI logs, or legacy Windows conhost with
/// no VT processing — raw escape bytes corrupt the output. `no_color`
/// (the `--no-color` flag or the `NO_COLOR` env var) still wins outright.
///
/// This does NOT attempt full conhost VT-enable plumbing (XP-K-03): the
/// observable fix for redirected output is simply not emitting escapes.
#[must_use]
pub fn ansi_enabled(is_terminal: bool, no_color: bool) -> bool {
    is_terminal && !no_color
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal gets ANSI unless the user opted out.
    #[test]
    fn terminal_with_colors_enabled_shows_ansi() {
        assert!(ansi_enabled(true, false));
    }

    /// Explicit `--no-color` / NO_COLOR always disables ANSI.
    #[test]
    fn no_color_always_wins() {
        assert!(!ansi_enabled(true, true));
        assert!(!ansi_enabled(false, true));
    }

    /// Redirected output never gets escapes, even without NO_COLOR —
    /// the XP-K-04 core decision (piped output, CI logs, legacy conhost).
    #[test]
    fn non_terminal_never_gets_ansi() {
        assert!(!ansi_enabled(false, false));
    }
}
