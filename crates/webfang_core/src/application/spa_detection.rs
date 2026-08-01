//! SPA detection — heuristic detection of JavaScript-rendered pages.
//!
//! Analyzes extracted content to identify pages that returned minimal content
//! after readability/fallback extraction, a common symptom of Single Page
//! Applications that render client-side.

/// Minimum character threshold for considering content "substantial".
/// Pages below this threshold after extraction likely require JS rendering.
pub const MIN_CONTENT_CHARS: usize = 50;

/// Result of SPA content detection analysis.
///
/// Contains diagnostic information about why a page was flagged
/// as potentially requiring JavaScript rendering.
#[derive(Debug, Clone)]
pub struct SpaDetectionResult {
    /// The URL that was analyzed
    pub url: String,
    /// Character count of the extracted content
    pub char_count: usize,
    /// Whether the HTML contains common SPA indicators
    pub has_spa_markers: bool,
}

/// Detect whether a page likely requires JavaScript rendering (SPA detection).
///
/// Analyzes extracted content to identify pages that returned minimal content
/// after readability/fallback extraction, which is a common symptom of
/// Single Page Applications that render client-side.
///
/// # Arguments
///
/// * `url` - The URL that was scraped
/// * `text_content` - The extracted text content (used for char count threshold)
/// * `raw_html` - The raw HTML source (used for SPA marker detection)
///
/// # Returns
///
/// * `Some(SpaDetectionResult)` if the page appears to be an SPA
/// * `None` if the content appears substantial enough
///
/// # Detection Heuristics
///
/// A page is flagged as potentially SPA-dependent when:
/// - Extracted content is below `MIN_CONTENT_CHARS` (50 chars)
pub fn detect_spa_content(
    url: &str,
    text_content: &str,
    raw_html: &str,
) -> Option<SpaDetectionResult> {
    let char_count = text_content.chars().count();

    if char_count >= MIN_CONTENT_CHARS {
        return None;
    }

    // Check for common SPA mount point markers in raw HTML (not stripped text)
    let has_spa_markers =
        raw_html.contains("<div id=\"root\">") || raw_html.contains("<div id=\"app\">");

    Some(SpaDetectionResult {
        url: url.to_string(),
        char_count,
        has_spa_markers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://spa.example.com";

    #[test]
    fn test_below_threshold_without_markers_is_flagged() {
        let result = detect_spa_content(URL, "", "<html><body></body></html>");
        let result = result.expect("empty content must be flagged as potential SPA");
        assert_eq!(result.url, URL);
        assert_eq!(result.char_count, 0);
        assert!(!result.has_spa_markers);
    }

    #[test]
    fn test_below_threshold_with_root_marker_sets_flag() {
        let result = detect_spa_content(URL, "loading", "<div id=\"root\"></div>")
            .expect("short content must be flagged");
        assert!(result.has_spa_markers, "root mount point must set marker");
    }

    #[test]
    fn test_below_threshold_with_app_marker_sets_flag() {
        let result = detect_spa_content(URL, "loading", "<div id=\"app\"></div>")
            .expect("short content must be flagged");
        assert!(result.has_spa_markers, "app mount point must set marker");
    }

    #[test]
    fn test_at_threshold_is_not_flagged() {
        let content = "a".repeat(MIN_CONTENT_CHARS);
        assert_eq!(content.chars().count(), MIN_CONTENT_CHARS);
        assert!(
            detect_spa_content(URL, &content, "<div id=\"root\"></div>").is_none(),
            "content at the threshold must be considered substantial"
        );
    }

    #[test]
    fn test_just_below_threshold_is_flagged() {
        let content = "a".repeat(MIN_CONTENT_CHARS - 1);
        let result = detect_spa_content(URL, &content, "<html></html>")
            .expect("content just below threshold must be flagged");
        assert_eq!(result.char_count, MIN_CONTENT_CHARS - 1);
    }

    #[test]
    fn test_above_threshold_is_not_flagged() {
        let content = "a".repeat(MIN_CONTENT_CHARS * 4);
        assert!(
            detect_spa_content(URL, &content, "<div id=\"root\"></div>").is_none(),
            "substantial content must not be flagged even with SPA markers"
        );
    }
}
