//! Selector service — builds MCP JSON responses with selector diagnostics.
//!
//! Keeps [`super::mod`] from growing past 1197 lines by delegating the
//! response-building logic for CSS selector scraping to this module.
//!
//! The [`build_scrape_response`] function takes the scrape results, the
//! [`ExtractResult`] from the selector extraction, and the original selector
//! parameter, and produces a [`serde_json::Value`] with:
//! - `results`: the scraped content
//! - `selector_applied`: whether a CSS selector was provided
//! - `selector_matched`: whether the selector matched elements
//! - `diagnostic`: optional diagnostic info (DOM structure + suggestions)

use webfang_core::domain::{ExtractResult, ScrapedContent};

/// Build the MCP scrape response JSON with selector metadata.
///
/// # Arguments
///
/// * `results` — The scraped content from `scrape_with_config`
/// * `extract_result` — The selector extraction result (Matched or Fallback)
/// * `selector` — The original CSS selector parameter (None if not provided)
///
/// # Returns
///
/// A JSON object with:
/// - `results`: array of scraped content
/// - `selector_applied`: `true` if a selector was provided
/// - `selector_matched`: `true` if the selector matched elements
/// - `diagnostic`: `null` when matched or no inspector; object on failure
#[must_use]
pub fn build_scrape_response(
    results: Vec<ScrapedContent>,
    extract_result: &ExtractResult,
    selector: &Option<String>,
) -> serde_json::Value {
    let selector_applied = selector.is_some();
    let selector_matched = extract_result.is_matched();

    let diagnostic = match extract_result {
        ExtractResult::Matched(_) => None,
        ExtractResult::Fallback { diagnostic, .. } => diagnostic.as_ref(),
    };

    serde_json::json!({
        "results": results,
        "selector_applied": selector_applied,
        "selector_matched": selector_matched,
        "diagnostic": diagnostic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use webfang_core::domain::{
        DomStructureReport, SelectorDiagnostic, SelectorErrorKind, SelectorSuggestion, ValidUrl,
    };

    /// Create a minimal ScrapedContent for testing.
    fn test_content(title: &str) -> ScrapedContent {
        ScrapedContent {
            title: title.to_owned(),
            content: String::new(),
            url: ValidUrl::parse("https://example.com").unwrap(),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
        }
    }

    // ── Test 1: Selector matches elements ──────────────────────────

    #[test]
    fn test_matched_with_selector() {
        let results = vec![test_content("Article Title")];
        let extract_result = ExtractResult::Matched("<article>Content</article>".to_owned());
        let selector = Some("main article".to_owned());

        let json = build_scrape_response(results, &extract_result, &selector);

        assert_eq!(
            json["selector_applied"], true,
            "selector_applied must be true"
        );
        assert_eq!(
            json["selector_matched"], true,
            "selector_matched must be true"
        );
        assert!(
            json["diagnostic"].is_null(),
            "diagnostic must be null on match"
        );
        assert_eq!(
            json["results"][0]["title"], "Article Title",
            "results must contain scraped content"
        );
    }

    // ── Test 2: Selector zero matches with diagnostic ─────────────

    #[test]
    fn test_fallback_with_diagnostic() {
        let results = vec![test_content("Full Page")];
        let report = DomStructureReport {
            element_count: 15,
            truncated: false,
            tag_counts: HashMap::from([("div".to_owned(), 5), ("p".to_owned(), 8)]),
            max_depth: 4,
            common_classes: vec![("main-content".to_owned(), 3)],
            common_ids: vec![("header".to_owned(), 1)],
        };
        let suggestions = vec![SelectorSuggestion {
            selector: ".main-content".to_owned(),
            score: 0.85,
        }];
        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::ZeroMatches,
            report,
            suggestions,
        };
        let extract_result = ExtractResult::Fallback {
            html: "<html>full page</html>".to_owned(),
            diagnostic: Some(diagnostic),
        };
        let selector = Some(".nonexistent".to_owned());

        let json = build_scrape_response(results, &extract_result, &selector);

        assert_eq!(json["selector_applied"], true);
        assert_eq!(json["selector_matched"], false);
        // serde serializes ZeroMatches as the string "ZeroMatches"
        assert_eq!(
            json["diagnostic"]["error_kind"], "ZeroMatches",
            "error_kind must be ZeroMatches"
        );
        assert_eq!(
            json["diagnostic"]["report"]["element_count"], 15,
            "report must contain element_count"
        );
        assert_eq!(
            json["diagnostic"]["report"]["tag_counts"]["div"], 5,
            "report must contain tag_counts"
        );
        assert_eq!(
            json["diagnostic"]["suggestions"][0]["selector"], ".main-content",
            "suggestions must contain closest match"
        );
        assert_eq!(
            json["diagnostic"]["suggestions"][0]["score"], 0.85,
            "suggestions must contain score"
        );
    }

    // ── Test 3: Invalid selector with diagnostic ──────────────────

    #[test]
    fn test_fallback_invalid_selector() {
        let results = vec![test_content("Page")];
        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::InvalidSelector("unexpected token".to_owned()),
            report: DomStructureReport::default(),
            suggestions: Vec::new(),
        };
        let extract_result = ExtractResult::Fallback {
            html: "<html>full</html>".to_owned(),
            diagnostic: Some(diagnostic),
        };
        let selector = Some("div >>> p".to_owned());

        let json = build_scrape_response(results, &extract_result, &selector);

        assert_eq!(json["selector_applied"], true);
        assert_eq!(json["selector_matched"], false);
        // serde serializes InvalidSelector("msg") as {"InvalidSelector": "msg"}
        assert_eq!(
            json["diagnostic"]["error_kind"]["InvalidSelector"], "unexpected token",
            "error_kind must be InvalidSelector with message"
        );
    }

    // ── Test 4: Fallback without diagnostic (no inspector) ─────────

    #[test]
    fn test_fallback_no_diagnostic() {
        let results = vec![test_content("Page")];
        let extract_result = ExtractResult::Fallback {
            html: "<html>full</html>".to_owned(),
            diagnostic: None,
        };
        let selector = Some(".missing".to_owned());

        let json = build_scrape_response(results, &extract_result, &selector);

        assert_eq!(json["selector_applied"], true);
        assert_eq!(json["selector_matched"], false);
        assert!(
            json["diagnostic"].is_null(),
            "diagnostic must be null when no inspector configured"
        );
    }

    // ── Test 5: No selector (backward compat) ─────────────────────

    #[test]
    fn test_no_selector_backward_compat() {
        let results = vec![test_content("Default Page")];
        // When no selector is provided, config.selector defaults to "body"
        // which always matches → ExtractResult::Matched
        let extract_result = ExtractResult::Matched("<body>default content</body>".to_owned());
        let selector: Option<String> = None;

        let json = build_scrape_response(results, &extract_result, &selector);

        assert_eq!(
            json["selector_applied"], false,
            "selector_applied must be false when no selector provided"
        );
        assert_eq!(
            json["selector_matched"], true,
            "selector_matched is true because body selector matches"
        );
        assert!(
            json["diagnostic"].is_null(),
            "diagnostic must be null on match"
        );
    }

    // ── Test 6: Empty results with matched selector ───────────────

    #[test]
    fn test_matched_empty_results() {
        let results: Vec<ScrapedContent> = Vec::new();
        let extract_result = ExtractResult::Matched(String::new());
        let selector = Some("img".to_owned());

        let json = build_scrape_response(results, &extract_result, &selector);

        assert_eq!(json["selector_applied"], true);
        assert_eq!(json["selector_matched"], true);
        assert!(
            json["results"].is_array(),
            "results must always be an array"
        );
        assert_eq!(
            json["results"].as_array().unwrap().len(),
            0,
            "results must be empty array"
        );
    }

    // ── Test 7: EmptyDocument error kind ───────────────────────────

    #[test]
    fn test_fallback_empty_document() {
        let results: Vec<ScrapedContent> = Vec::new();
        let diagnostic = SelectorDiagnostic {
            error_kind: SelectorErrorKind::EmptyDocument,
            report: DomStructureReport::default(),
            suggestions: Vec::new(),
        };
        let extract_result = ExtractResult::Fallback {
            html: String::new(),
            diagnostic: Some(diagnostic),
        };
        let selector = Some("article".to_owned());

        let json = build_scrape_response(results, &extract_result, &selector);

        assert_eq!(json["selector_applied"], true);
        assert_eq!(json["selector_matched"], false);
        // serde serializes EmptyDocument as the string "EmptyDocument"
        assert_eq!(
            json["diagnostic"]["error_kind"], "EmptyDocument",
            "error_kind must be EmptyDocument"
        );
    }
}
