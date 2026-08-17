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

    // Check for common SPA mount point markers in raw HTML (not stripped text).
    // The char gate above runs first, so server-rendered pages that also embed
    // __NEXT_DATA__ / __INITIAL_STATE__ payloads but return substantial content
    // are never flagged (SD-1: single 50-char threshold).
    let has_spa_markers = raw_html.contains("<div id=\"root\">")
        || raw_html.contains("<div id=\"app\">")
        || raw_html.contains("__NEXT_DATA__")
        || raw_html.contains("window.__INITIAL_STATE__");

    Some(SpaDetectionResult {
        url: url.to_string(),
        char_count,
        has_spa_markers,
    })
}

/// Shared minimum-content guard for BOTH extraction funnels (#706).
///
/// Applies [`detect_spa_content`] to the freshly extracted content and fails
/// the scrape with a typed [`crate::error::ScraperError::ExtractionFailed`]
/// instead of returning `Ok` on near-empty output. Both extraction funnels —
/// the MCP funnel (`build_scraped_content`) and the CLI funnel
/// (`extract_content`) — call this after extraction, so JS-shell pages fail
/// honestly instead of producing near-empty Markdown.
///
/// The Spanish `reason` distinguishes pages that carry SPA markers (likely
/// requiring JavaScript rendering) from pages that simply returned very
/// little content. 50 (`MIN_CONTENT_CHARS`) stays the ONLY threshold.
///
/// # Errors
///
/// Returns [`crate::error::ScraperError::ExtractionFailed`] when the extracted
/// text is below [`MIN_CONTENT_CHARS`] characters.
pub fn validate_min_content(
    url: &str,
    text_content: &str,
    raw_html: &str,
    correlation: &crate::domain::CorrelationId,
) -> Result<(), crate::error::ScraperError> {
    if let Some(spa_info) = detect_spa_content(url, text_content, raw_html) {
        let reason = if spa_info.has_spa_markers {
            format!(
                "contenido insuficiente ({} caracteres) con marcadores SPA detectados — la página requiere renderizado de JavaScript",
                spa_info.char_count
            )
        } else {
            format!(
                "contenido insuficiente ({} caracteres) — la página devolvió muy poco contenido extraíble",
                spa_info.char_count
            )
        };
        let err = crate::error::ScraperError::ExtractionFailed {
            url: url.to_string(),
            reason,
        };
        crate::infrastructure::observability::log_scrape_error(
            &err,
            url,
            "extract",
            Some(correlation),
            "minimum-content guard",
        );
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::CorrelationId;

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
    fn test_below_threshold_with_next_data_marker_sets_flag() {
        let result = detect_spa_content(
            URL,
            "loading",
            "<html><body><script id=\"__NEXT_DATA__\" type=\"application/json\">{}</script></body></html>",
        )
        .expect("short content must be flagged");
        assert!(
            result.has_spa_markers,
            "__NEXT_DATA__ payload must set marker"
        );
    }

    #[test]
    fn test_below_threshold_with_initial_state_marker_sets_flag() {
        let result = detect_spa_content(
            URL,
            "loading",
            "<html><body><script>window.__INITIAL_STATE__ = {};</script></body></html>",
        )
        .expect("short content must be flagged");
        assert!(
            result.has_spa_markers,
            "window.__INITIAL_STATE__ must set marker"
        );
    }

    #[test]
    fn test_gated_markers_at_threshold_is_not_flagged() {
        // Server-rendered __NEXT_DATA__ pages with substantial content must
        // NOT be flagged: the char gate fires BEFORE the marker check.
        let content = "a".repeat(MIN_CONTENT_CHARS);
        for marker_html in [
            "<html><script id=\"__NEXT_DATA__\">{}</script></html>",
            "<html><script>window.__INITIAL_STATE__ = {};</script></html>",
        ] {
            assert!(
                detect_spa_content(URL, &content, marker_html).is_none(),
                "content at/above threshold must never be flagged, even with markers"
            );
        }
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

    // --- validate_min_content: the shared two-funnel guard ---

    #[test]
    fn test_validate_min_content_ok_at_threshold() {
        let correlation = CorrelationId::new();
        let content = "a".repeat(MIN_CONTENT_CHARS);
        assert!(
            validate_min_content(URL, &content, "<div id=\"root\"></div>", &correlation).is_ok(),
            "content at the threshold must pass the guard"
        );
    }

    #[test]
    fn test_validate_min_content_ok_above_threshold() {
        let correlation = CorrelationId::new();
        let content = "a".repeat(MIN_CONTENT_CHARS * 4);
        assert!(
            validate_min_content(URL, &content, "<html></html>", &correlation).is_ok(),
            "substantial content must pass the guard"
        );
    }

    /// Assert that `err` is the typed `ExtractionFailed` for [`URL`] and
    /// return its Spanish reason for further assertions.
    fn expect_extraction_failed(err: &crate::error::ScraperError) -> &str {
        match err {
            crate::error::ScraperError::ExtractionFailed { url, reason } => {
                assert_eq!(url, URL, "url must be preserved on the error");
                reason
            },
            other => panic!("expected ExtractionFailed, got: {other:?}"),
        }
    }

    #[test]
    fn test_validate_min_content_err_with_markers_mentions_js() {
        let correlation = CorrelationId::new();
        let err = validate_min_content(URL, "loading", "<div id=\"root\"></div>", &correlation)
            .expect_err("sub-threshold marker content must fail the guard");
        let reason = expect_extraction_failed(&err);
        assert!(
            reason.contains("contenido insuficiente"),
            "Spanish reason must state insufficient content: {reason}"
        );
        assert!(
            reason.contains("renderizado de JavaScript"),
            "marker variant must mention JavaScript rendering: {reason}"
        );
    }

    #[test]
    fn test_validate_min_content_err_without_markers() {
        let correlation = CorrelationId::new();
        let err = validate_min_content(URL, "", "<html><body></body></html>", &correlation)
            .expect_err("empty content must fail the guard");
        let reason = expect_extraction_failed(&err);
        assert!(
            reason.contains("contenido insuficiente"),
            "Spanish reason must state insufficient content: {reason}"
        );
        assert!(
            !reason.contains("renderizado de JavaScript"),
            "no-marker variant must not claim JS requirement: {reason}"
        );
    }
}
