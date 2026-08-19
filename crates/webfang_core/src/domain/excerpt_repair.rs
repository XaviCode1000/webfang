//! Excerpt byline repair — shared domain invariant (#762 / #800).
//!
//! Legible (Mozilla Readability port) captures the first author node
//! (`<small itemprop="author">`) as the page byline and strips it from the
//! body, leaving only the wrapper's `by ` prefix and the sibling `(about)`
//! anchor behind — the excerpt then ships `… by  (about)` (doubled space
//! where the node was).
//!
//! This is a PURE string invariant (static regex + whitespace collapse) with
//! ZERO IO, so it lives at the innermost `domain` layer and is reachable from
//! every layer that needs it (infrastructure→domain, application→domain) without
//! violating the inward-only dependency rule.

use std::borrow::Cow;
use std::sync::LazyLock;

/// Collapse whitespace runs in `text` to single spaces, zero-cost when clean.
///
/// Readability excerpts can carry extraction artifacts such as doubled
/// spaces ("...by  (about)", RIESGO-OBS-001). Borrowing the input when it
/// is already clean keeps the common path allocation-free; only dirty
/// excerpts pay for a cleaned clone.
pub(crate) fn normalize_whitespace(text: &str) -> Cow<'_, str> {
    let needs_cleanup = text.chars().any(|c| c.is_whitespace() && c != ' ') || text.contains("  ");
    if needs_cleanup {
        Cow::Owned(text.split_whitespace().collect::<Vec<_>>().join(" "))
    } else {
        Cow::Borrowed(text)
    }
}

/// A byline fragment whose author name is missing: `by (about)`, `by (more)…`.
///
/// Compile-time-constant pattern, so `expect` documents a true invariant
/// (same convention as the `author_extractor` static selectors).
#[allow(clippy::expect_used)]
static EMPTY_BYLINE_FRAGMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\bby\s+\([^)]*\)")
        // LCOV_EXCL_LINE defensive: fixed pattern, compile cannot fail
        .expect("BUG: invalid byline fragment regex")
});

/// Repair a residual empty-byline fragment in `excerpt` (#762 / #800).
///
/// When the author name could be resolved (the extractor cascade found it),
/// the fragment is completed to `by <author>`; otherwise it is dropped —
/// a `by` with no name is noise, not information. The removal can resurrect
/// doubled spaces or trailing seams, so the result is re-normalized.
///
/// The repair is IDEMPOTENT: re-applying on already-repaired or clean text is
/// a byte no-op, so the Markdown frontmatter path (which repairs again) emits
/// byte-identical output to before this module existed. English `by (...)` only;
/// the Spanish `por (...)` variant is explicitly OUT of scope (follow-up).
pub(crate) fn repair_empty_byline(excerpt: &str, author: Option<&str>) -> String {
    let repaired: Cow<'_, str> =
        EMPTY_BYLINE_FRAGMENT.replace_all(excerpt, |_: &regex::Captures| match author {
            Some(name) => format!("by {name}"),
            None => String::new(),
        });
    // Re-normalize: dropping the fragment can leave `…  …` or trailing space.
    normalize_whitespace(repaired.trim()).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clean excerpts are borrowed untouched (zero-cost path) and the repair
    /// returns them byte-identical.
    #[test]
    fn normalize_whitespace_borrows_clean_text() {
        let out = normalize_whitespace("a clean excerpt");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "a clean excerpt");
    }

    /// Doubled spaces (the Readability "...by  (about)" artifact) collapse
    /// to single spaces via the owned path.
    #[test]
    fn normalize_whitespace_collapses_double_spaces() {
        let out = normalize_whitespace("...by  (about)");
        assert!(matches!(out, Cow::Owned(_)));
        assert_eq!(out, "...by (about)");
    }

    /// Tabs and newlines collapse to single spaces too.
    #[test]
    fn normalize_whitespace_collapses_tabs_and_newlines() {
        let out = normalize_whitespace("word\t\nword");
        assert_eq!(out, "word word");
    }

    /// The frontmatter serializes the normalized excerpt. Since #762 the
    /// empty-byline fragment is DROPPED when no author is available (the
    /// whitespace collapse from #695 still runs as part of the repair pass).
    #[test]
    fn excerpt_normalizes_whitespace() {
        let repaired = repair_empty_byline("...by  (about)", None);
        assert_eq!(repaired, "...");
        assert!(!repaired.contains("by (about)"));
        assert!(!repaired.contains("by  (about)"));
    }

    /// With a resolved author, the residual `by (about)` is completed to
    /// `by <author>` instead of shipping an empty name slot.
    #[test]
    fn repair_empty_byline_completes_with_author() {
        let repaired = repair_empty_byline(
            "“The world… changing our thinking.” by  (about)",
            Some("Albert Einstein"),
        );
        assert_eq!(
            repaired,
            "“The world… changing our thinking.” by Albert Einstein"
        );
    }

    /// Without an author, the fragment is dropped entirely — a `by` with no
    /// name is noise. The trailing seam is trimmed.
    #[test]
    fn repair_empty_byline_drops_without_author() {
        let repaired = repair_empty_byline("“The world… changing our thinking.” by (about)", None);
        assert_eq!(repaired, "“The world… changing our thinking.”");
    }

    /// Text that does not match stays untouched (after normalization pass).
    #[test]
    fn repair_empty_byline_leaves_clean_excerpt_alone() {
        let repaired = repair_empty_byline("A clean excerpt without scars", Some("Someone"));
        assert_eq!(repaired, "A clean excerpt without scars");
    }

    /// A real "by" phrase with an actual name is NOT mistaken for a scar.
    #[test]
    fn repair_empty_byline_keeps_real_by_phrases() {
        let repaired = repair_empty_byline("Written by Jane Austen", Some("Jane Austen"));
        assert_eq!(repaired, "Written by Jane Austen");
    }

    /// A bare `by ` with no parenthetical fragment is left untouched — the
    /// pattern only matches `by (...)`.
    #[test]
    fn repair_empty_byline_leaves_bare_by_prefix_alone() {
        let repaired = repair_empty_byline("Some text by  trailing", Some("Nobody"));
        assert_eq!(repaired, "Some text by trailing");
    }

    /// Empty input passes through safely with no panic and no residue.
    #[test]
    fn repair_empty_byline_handles_empty_input() {
        assert_eq!(repair_empty_byline("", None), "");
        assert_eq!(repair_empty_byline("", Some("A")), "");
    }

    /// The Spanish `por (...)` variant is NOT matched (documented follow-up);
    /// it must pass through unchanged.
    #[test]
    fn repair_empty_byline_leaves_spanish_por_untouched() {
        let repaired = repair_empty_byline("citado por (acerca de)", None);
        assert_eq!(repaired, "citado por (acerca de)");
    }

    /// Idempotence: repairing already-repaired text is a byte no-op, so the
    /// Markdown frontmatter double-apply path stays byte-identical.
    #[test]
    fn repair_empty_byline_is_idempotent() {
        let once = repair_empty_byline(
            "“The world… changing our thinking.” by  (about)",
            Some("Albert Einstein"),
        );
        let twice = repair_empty_byline(&once, Some("Albert Einstein"));
        assert_eq!(once, twice);
        // And clean text stays clean under repeated repair.
        let clean = repair_empty_byline("A clean excerpt.", Some("Someone"));
        assert_eq!(repair_empty_byline(&clean, Some("Someone")), clean);
    }
}
