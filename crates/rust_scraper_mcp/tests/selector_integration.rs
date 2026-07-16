//! Integration tests for CSS selector scraping with diagnostics.
//!
//! Tests the full pipeline: HTML fixture → DefaultDomInspector →
//! build_scrape_response → JSON output with selector metadata.
//!
//! These tests verify that the MCP response includes:
//! - `selector_applied`: whether a selector was provided
//! - `selector_matched`: whether the selector matched elements
//! - `diagnostic`: DOM structure report + suggestions on failure

use rust_scraper_core::domain::{
    DomInspectorPort, ExtractResult, ScrapedContent, SelectorDiagnostic, SelectorErrorKind,
    ValidUrl,
};
use rust_scraper_core::infrastructure::scraper::dom_inspector::DefaultDomInspector;
use rust_scraper_mcp::mcp_server::selector_service::build_scrape_response;

/// Load the HTML fixture for testing.
fn fixture_html() -> String {
    std::fs::read_to_string("tests/fixtures/selector_test_page.html").expect("read fixture HTML")
}

/// Create a minimal ScrapedContent for testing.
fn make_content(title: &str) -> ScrapedContent {
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

/// Extract elements matching a CSS selector from HTML.
/// Returns `Ok(html)` if matched, `Err(SelectorErrorKind)` on failure.
fn try_select(html: &str, selector: &str) -> Result<String, SelectorErrorKind> {
    let document = scraper::Html::parse_document(html);
    match scraper::Selector::parse(selector) {
        Ok(sel) => {
            let matching: Vec<String> = document.select(&sel).map(|el| el.html()).collect();
            if matching.is_empty() {
                Err(SelectorErrorKind::ZeroMatches)
            } else {
                Ok(matching.join("\n"))
            }
        },
        Err(e) => Err(SelectorErrorKind::InvalidSelector(e.to_string())),
    }
}

/// Build a real diagnostic using DefaultDomInspector on the fixture HTML.
fn build_real_diagnostic(
    html: &str,
    selector: &str,
    error_kind: SelectorErrorKind,
) -> SelectorDiagnostic {
    let inspector = DefaultDomInspector::new();
    let document = scraper::Html::parse_document(html);
    let report = inspector.inspect(&document);
    let suggestions = inspector.suggest(&document, selector);
    SelectorDiagnostic {
        error_kind,
        report,
        suggestions,
    }
}

/// Pretty-print a serde_json::Value for snapshot comparison.
fn pretty(json: serde_json::Value) -> String {
    serde_json::to_string_pretty(&json).unwrap_or_else(|_| "serialization failed".into())
}

// ── Test 1: Selector provided and matches elements ──────────────

#[test]
fn test_scrape_with_matching_selector() {
    let html = fixture_html();
    let selector = "main article".to_owned();
    let matched_html = try_select(&html, &selector).expect("selector should match");
    let extract_result = ExtractResult::Matched(matched_html);
    let results = vec![make_content("Article Title")];

    let json = build_scrape_response(results, &extract_result, &Some(selector));

    insta::assert_snapshot!("matching_selector", pretty(json));
}

// ── Test 2: Selector provided but 0 matches ─────────────────────

#[test]
fn test_scrape_with_zero_match_selector() {
    let html = fixture_html();
    let selector = ".nonexistent".to_owned();
    let error_kind = try_select(&html, &selector).expect_err("should be zero matches");
    let diagnostic = build_real_diagnostic(&html, &selector, error_kind);
    let extract_result = ExtractResult::Fallback {
        html: html.clone(),
        diagnostic: Some(diagnostic),
    };
    let results = vec![make_content("Full Page")];

    let json = build_scrape_response(results, &extract_result, &Some(selector));

    insta::assert_snapshot!("zero_match_selector", pretty(json));
}

// ── Test 3: Invalid CSS selector ─────────────────────────────────

#[test]
fn test_scrape_with_invalid_selector() {
    let html = fixture_html();
    let selector = "div >>> p".to_owned();
    let error_kind = try_select(&html, &selector).expect_err("should be invalid selector");
    let diagnostic = build_real_diagnostic(&html, &selector, error_kind);
    let extract_result = ExtractResult::Fallback {
        html: html.clone(),
        diagnostic: Some(diagnostic),
    };
    let results = vec![make_content("Page")];

    let json = build_scrape_response(results, &extract_result, &Some(selector));

    insta::assert_snapshot!("invalid_selector", pretty(json));
}

// ── Test 4: No selector provided (backward compat) ──────────────

#[test]
fn test_scrape_without_selector_backward_compat() {
    let html = fixture_html();
    // When no selector is provided, config.selector defaults to "body"
    // which always matches → ExtractResult::Matched
    let extract_result = ExtractResult::Matched(html);
    let results = vec![make_content("Default Page")];
    let selector: Option<String> = None;

    let json = build_scrape_response(results, &extract_result, &selector);

    insta::assert_snapshot!("no_selector_backward_compat", pretty(json));
}

// ── Test 5: Zero matches without inspector (diagnostic null) ─────

#[test]
fn test_scrape_zero_match_no_inspector() {
    let html = fixture_html();
    let selector = ".missing-class".to_owned();
    // No inspector configured → diagnostic: null
    let extract_result = ExtractResult::Fallback {
        html: html.clone(),
        diagnostic: None,
    };
    let results = vec![make_content("Page")];

    let json = build_scrape_response(results, &extract_result, &Some(selector));

    assert_eq!(json["selector_applied"], true);
    assert_eq!(json["selector_matched"], false);
    assert!(
        json["diagnostic"].is_null(),
        "diagnostic must be null without inspector"
    );
}

// ── Test 6: Empty HTML page → EmptyDocument diagnostic ──────────

#[test]
fn test_scrape_with_empty_page() {
    // Simulate an empty HTML page (server returns empty body).
    // extract_with_selector would produce EmptyDocument (not ZeroMatches)
    // because html.trim().is_empty() triggers the early return.
    let html = "";
    let selector = "article".to_owned();
    let diagnostic = build_real_diagnostic(html, &selector, SelectorErrorKind::EmptyDocument);
    let extract_result = ExtractResult::Fallback {
        html: html.to_owned(),
        diagnostic: Some(diagnostic),
    };
    let results = vec![make_content("Empty Page")];

    let json = build_scrape_response(results, &extract_result, &Some(selector));

    assert_eq!(
        json["selector_applied"], true,
        "selector_applied must be true when a selector is provided"
    );
    assert_eq!(
        json["selector_matched"], false,
        "selector_matched must be false for an empty page"
    );
    assert_eq!(
        json["diagnostic"]["error_kind"], "EmptyDocument",
        "error_kind must be EmptyDocument for empty HTML"
    );

    insta::assert_snapshot!("empty_page", pretty(json));
}
