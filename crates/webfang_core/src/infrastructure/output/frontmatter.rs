//! YAML frontmatter generation
//!
//! Generates YAML frontmatter for Markdown files with metadata:
//! - Title
//! - URL
//! - Date (publication or scrape date)
//! - Author (if available)
//! - Excerpt (if available)
//! - Tags (if available, for Obsidian compatibility)
//! - Rich metadata (word count, reading time, language, content type, status)

use std::borrow::Cow;
use std::sync::LazyLock;

use chrono::Utc;
use serde::Serialize;

/// Collapse whitespace runs in `text` to single spaces, zero-cost when clean.
///
/// Readability excerpts can carry extraction artifacts such as doubled
/// spaces ("...by  (about)", RIESGO-OBS-001). Borrowing the input when it
/// is already clean keeps the common path allocation-free; only dirty
/// excerpts pay for a cleaned clone.
fn normalize_whitespace(text: &str) -> Cow<'_, str> {
    let needs_cleanup = text.chars().any(|c| c.is_whitespace() && c != ' ') || text.contains("  ");
    if needs_cleanup {
        Cow::Owned(text.split_whitespace().collect::<Vec<_>>().join(" "))
    } else {
        Cow::Borrowed(text)
    }
}

/// A byline fragment whose author name is missing: `by (about)`, `by (more)…`.
///
/// Reproduced from quotes.toscrape.com (#762): Readability captures the
/// author NAME node (`<small itemprop="author">`) as the page byline and
/// strips it from the body, leaving only the wrapper's `by ` prefix and the
/// sibling `(about)` anchor behind — the excerpt then ships `… by (about)`.
/// Compile-time-constant pattern, so `expect` documents a true invariant
/// (same convention as the `author_extractor` static selectors).
#[allow(clippy::expect_used)]
static EMPTY_BYLINE_FRAGMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\bby\s+\([^)]*\)")
        // LCOV_EXCL_LINE defensive: fixed pattern, compile cannot fail
        .expect("BUG: invalid byline fragment regex")
});

/// Repair a residual empty-byline fragment in `excerpt` (#762).
///
/// When the author name could be resolved (the extractor cascade found it),
/// the fragment is completed to `by <author>`; otherwise it is dropped —
/// a `by` with no name is noise, not information. The removal can resurrect
/// doubled spaces or trailing seams, so the result is re-normalized.
fn repair_empty_byline(excerpt: &str, author: Option<&str>) -> String {
    let repaired: Cow<'_, str> =
        EMPTY_BYLINE_FRAGMENT.replace_all(excerpt, |_: &regex::Captures| match author {
            Some(name) => format!("by {name}"),
            None => String::new(),
        });
    // Re-normalize: dropping the fragment can leave `…  …` or trailing space.
    normalize_whitespace(repaired.trim()).into_owned()
}

/// Frontmatter data structure
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Frontmatter {
    /// Article/page title
    title: String,
    /// Original URL
    #[serde(skip_serializing_if = "String::is_empty")]
    url: String,
    /// Publication/scrape date (YYYY-MM-DD)
    date: String,
    /// Author name (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
    /// Excerpt/summary (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    excerpt: Option<String>,
    /// Tags for Obsidian (if available)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
    /// Word count (if rich metadata enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    word_count: Option<usize>,
    /// Reading time in minutes (if rich metadata enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    reading_time: Option<usize>,
    /// Detected language (if rich metadata enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    /// Content type (if rich metadata enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    content_type: Option<String>,
    /// Scrape date (ISO 8601) (if rich metadata enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    scrape_date: Option<String>,
    /// Source URL (if rich metadata enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    /// Status (if rich metadata enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}

/// Generate YAML frontmatter for a markdown file (basic version).
///
/// This is a wrapper around `generate_with_metadata` for backward compatibility.
pub fn generate(
    title: &str,
    url: &str,
    date: Option<&str>,
    author: Option<&str>,
    excerpt: Option<&str>,
    tags: &[String],
) -> String {
    generate_with_metadata(title, url, date, author, excerpt, tags, None)
}

/// Generate YAML frontmatter with optional rich metadata.
///
/// # Arguments
/// * `title` - Article/page title
/// * `url` - Original URL
/// * `date` - Publication date (optional, uses current date if None)
/// * `author` - Author name (optional)
/// * `excerpt` - Excerpt/summary (optional)
/// * `tags` - Tags for Obsidian (optional, empty slice for no tags)
/// * `rich_meta` - Optional rich metadata (word count, reading time, etc.)
///
/// # Returns
/// YAML string without the surrounding `---` delimiters
pub fn generate_with_metadata(
    title: &str,
    url: &str,
    date: Option<&str>,
    author: Option<&str>,
    excerpt: Option<&str>,
    tags: &[String],
    rich_meta: Option<&crate::infrastructure::obsidian::ObsidianRichMetadata>,
) -> String {
    let fm = Frontmatter {
        title: title.to_string(),
        url: url.to_string(),
        date: date
            .map(|s| s.to_string())
            .unwrap_or_else(|| Utc::now().format("%Y-%m-%d").to_string()),
        author: author.map(|s| s.to_string()),
        excerpt: excerpt.map(|s| repair_empty_byline(s, author)),
        tags: tags.to_vec(),
        word_count: rich_meta.map(|m| m.word_count),
        reading_time: rich_meta.map(|m| m.reading_time),
        language: rich_meta.as_ref().and_then(|m| m.language.clone()),
        content_type: rich_meta.map(|m| m.content_type.clone()),
        scrape_date: rich_meta.as_ref().map(|m| m.scrape_date.clone()),
        source: rich_meta.as_ref().map(|m| m.source.clone()),
        status: rich_meta.as_ref().map(|m| m.status.clone()),
    };

    serde_yaml::to_string(&fm).unwrap_or_else(|_| String::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_with_all_fields() {
        let fm = generate(
            "Test Title",
            "https://example.com",
            Some("2024-01-15"),
            Some("John Doe"),
            Some("Test excerpt"),
            &["tag1".to_string(), "tag2".to_string()],
        );

        assert!(fm.contains("title: Test Title"));
        assert!(fm.contains("url: https://example.com"));
        assert!(fm.contains("date:")); // Date format may vary
        assert!(fm.contains("author: John Doe"));
        assert!(fm.contains("excerpt: Test excerpt"));
        assert!(fm.contains("tags:"));
        assert!(fm.contains("tag1"));
        assert!(fm.contains("tag2"));
    }

    #[test]
    fn test_generate_with_auto_date() {
        let fm = generate("Test", "https://example.com", None, None, None, &[]);

        assert!(fm.contains("title: Test"));
        assert!(fm.contains("url: https://example.com"));
        // Date should be today (format may vary)
        assert!(fm.contains("date:"));
        assert!(!fm.contains("author"));
        assert!(!fm.contains("excerpt"));
        assert!(!fm.contains("tags:"));
    }

    #[test]
    fn test_generate_minimal() {
        let fm = generate("Minimal", "https://minimal.com", None, None, None, &[]);

        assert!(fm.contains("title: Minimal"));
        assert!(fm.contains("url: https://minimal.com"));
    }

    #[test]
    fn test_generate_with_tags() {
        let fm = generate(
            "Tagged",
            "https://example.com",
            None,
            None,
            None,
            &["scraped".to_string(), "web-dev".to_string()],
        );

        assert!(fm.contains("tags:"));
        assert!(fm.contains("scraped"));
        assert!(fm.contains("web-dev"));
    }

    #[test]
    fn test_generate_empty_tags_not_serialized() {
        let fm = generate("No Tags", "https://example.com", None, None, None, &[]);

        // Empty tags should not appear in output
        assert!(!fm.contains("tags:"));
    }

    #[test]
    fn test_generate_with_rich_metadata() {
        let rich_meta = crate::infrastructure::obsidian::ObsidianRichMetadata {
            word_count: 1234,
            reading_time: 7,
            language: Some("eng".to_string()), // whatlang returns ISO 639-2
            content_type: "article".to_string(),
            scrape_date: "2026-04-03T12:00:00Z".to_string(),
            source: "https://example.com/article".to_string(),
            status: "unread".to_string(),
        };

        let fm = generate_with_metadata(
            "Test Article",
            "https://example.com/article",
            Some("2026-04-03"),
            None,
            None,
            &[],
            Some(&rich_meta),
        );

        // Frontmatter uses camelCase for rich metadata fields
        assert!(fm.contains("wordCount: 1234"));
        assert!(fm.contains("readingTime: 7"));
        assert!(fm.contains("language: eng"));
        assert!(fm.contains("contentType: article"));
        assert!(fm.contains("status: unread"));
    }

    #[test]
    fn test_generate_without_rich_metadata_backward_compatible() {
        // When no rich metadata, these fields should not appear
        let fm = generate("Test Article", "https://example.com", None, None, None, &[]);

        assert!(!fm.contains("wordCount"));
        assert!(!fm.contains("readingTime"));
        assert!(!fm.contains("language"));
        assert!(!fm.contains("contentType"));
        assert!(!fm.contains("status"));
    }

    // ====================================================================
    // #695 — excerpt whitespace normalization (RIESGO-OBS-001)
    // ====================================================================

    /// Clean excerpts are borrowed untouched (zero-cost path).
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
    fn test_generate_normalizes_excerpt_whitespace() {
        let fm = generate(
            "Title",
            "https://example.com",
            Some("2024-01-15"),
            None,
            Some("...by  (about)"),
            &[],
        );
        assert!(fm.contains("excerpt: '...'"), "got: {fm}");
        assert!(!fm.contains("by (about)"), "got: {fm}");
        assert!(!fm.contains("by  (about)"), "got: {fm}");
    }

    // ====================================================================
    // #762 — empty byline fragment repair
    // ====================================================================

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

    /// End-to-end: generate() emits the repaired excerpt in the YAML.
    /// serde_yaml switches to double quotes when the string carries smart
    /// quotes, so assert on the completed fragment itself.
    #[test]
    fn test_generate_repairs_byline_fragment_in_excerpt() {
        let fm = generate(
            "Quotes",
            "https://quotes.toscrape.com",
            Some("2026-08-17"),
            Some("Albert Einstein"),
            Some("“…changing our thinking.” by  (about)"),
            &[],
        );
        assert!(
            fm.contains("…changing our thinking.” by Albert Einstein"),
            "excerpt must be completed with the author, got: {fm}"
        );
        assert!(
            !fm.contains("by (about)") && !fm.contains("by  (about)"),
            "the empty-byline fragment must not survive: {fm}"
        );
    }
}
