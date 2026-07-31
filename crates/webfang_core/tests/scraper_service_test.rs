use std::collections::HashMap;

use webfang_core::application::http_client::{HttpError, HttpResponse};
use webfang_core::application::scraper_service::{
    detect_spa_content, extract_with_selector, scrape_multiple_with_limit, scrape_with_config,
    scrape_with_readability, MAX_INSTRUMENTED_BODY_SIZE, MIN_CONTENT_CHARS,
};
use webfang_core::domain::{DomInspectorPort, ExtractResult, SelectorErrorKind};
use webfang_core::{ScraperConfig, ScraperError};

// --- Shared mock HTTP client (tests/common/mock_http.rs, Item 2 Tier-2) ---

#[path = "common/mock_http.rs"]
mod mock_http;
use crate::mock_http::MockHttpClient;

// =====================================================================
// scrape_with_config tests
// =====================================================================

#[tokio::test]
async fn test_scrape_with_config_invalid_url() {
    let url = url::Url::parse("https://invalid-host-that-does-not-exist-12345.com").unwrap();
    let config = ScraperConfig::default();
    let mock = MockHttpClient::new().with_response(
        url.as_str(),
        Err(HttpError::Connection("no route to host".into())),
    );

    let result = scrape_with_config(&mock, &url, &config, None, None, None).await;
    assert!(result.is_err(), "connection error should propagate as Err");
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_scrape_with_config_returns_outcome() {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
<article>
<h1>Main Heading</h1>
<p>This is the content of the article. It has enough text to be extracted by Readability.</p>
</article>
</body>
</html>"#;

    let url = url::Url::parse("https://example.com").unwrap();
    let mock = MockHttpClient::new().with_ok_response(url.as_str(), html);
    let config = ScraperConfig::default();

    let outcome = scrape_with_config(&mock, &url, &config, None, None, None)
        .await
        .expect("mock HTML should succeed");
    assert!(
        !outcome.results.is_empty(),
        "should have at least one scraped result"
    );
    assert!(
        outcome.extract_result.is_matched(),
        "default 'body' selector should produce Matched extract_result"
    );
    assert!(
        !outcome.results[0].content.is_empty(),
        "scraped content should not be empty"
    );
}

// =====================================================================
// ScraperConfig tests
// =====================================================================

#[test]
fn test_scraper_config_concurrency_default() {
    let config = ScraperConfig::default();
    assert_eq!(config.scraper_concurrency, 3);
}

#[test]
fn test_scraper_config_concurrency_custom() {
    let config = ScraperConfig::default().with_scraper_concurrency(5);
    assert_eq!(config.scraper_concurrency, 5);
}

// =====================================================================
// detect_spa_content tests
// =====================================================================

#[test]
fn test_detect_spa_content_below_threshold() {
    let result = detect_spa_content("https://example.com", "", "");
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.char_count, 0);
    assert_eq!(result.url, "https://example.com");
}

#[test]
fn test_detect_spa_content_above_threshold() {
    let result = detect_spa_content(
        "https://example.com",
        "This is a substantial content that exceeds the minimum threshold of 50 characters easily.",
        "<html><body>Content</body></html>",
    );
    assert!(result.is_none());
}

#[test]
fn test_detect_spa_content_spa_markers() {
    // SPA markers should be detected in raw_html, not text_content
    let result = detect_spa_content(
        "https://spa.example.com",
        "minimal text",
        "<div id=\"root\"></div>",
    );
    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.has_spa_markers);
}

#[test]
fn test_detect_spa_content_spa_markers_app() {
    // Test the "app" marker as well
    let result = detect_spa_content(
        "https://spa.example.com",
        "minimal text",
        "<div id=\"app\"></div>",
    );
    assert!(result.is_some());
    let result = result.unwrap();
    assert!(result.has_spa_markers);
}

#[test]
fn test_detect_spa_content_no_spa_markers() {
    // No SPA markers in raw HTML should result in has_spa_markers = false
    let content = "a".repeat(49);
    let result = detect_spa_content(
        "https://example.com",
        &content,
        "<html><body>Some content</body></html>",
    );
    assert!(result.is_some());
    let result = result.unwrap();
    assert!(!result.has_spa_markers);
}

#[test]
fn test_detect_spa_content_just_below_threshold() {
    // 49 chars - just below threshold
    let content = "a".repeat(49);
    let result = detect_spa_content(
        "https://example.com",
        &content,
        "<html><body>Some content</body></html>",
    );
    assert!(result.is_some());
    assert_eq!(result.unwrap().char_count, 49);
}

#[test]
fn test_detect_spa_content_at_threshold() {
    // Exactly 50 chars - at threshold, should NOT trigger
    let content = "a".repeat(50);
    let result = detect_spa_content(
        "https://example.com",
        &content,
        "<html><body>Some content</body></html>",
    );
    assert!(result.is_none());
}

#[test]
fn test_detect_spa_content_differentiated_warnings() {
    // Test: SPA markers detected - should have has_spa_markers = true
    let result_with_markers =
        detect_spa_content("https://example.com", "", "<div id=\"root\"></div>");
    assert!(result_with_markers.is_some());
    assert!(result_with_markers.unwrap().has_spa_markers);

    // Test: minimal content without SPA markers - should have has_spa_markers = false
    let result_without_markers =
        detect_spa_content("https://example.com", "", "<html><body></body></html>");
    assert!(result_without_markers.is_some());
    assert!(!result_without_markers.unwrap().has_spa_markers);
}

// =====================================================================
// Mock-based tests for HttpClientPort integration
// =====================================================================

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_html_returns_title_and_content() {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
<article>
<h1>Main Heading</h1>
<p>This is the content of the article. It has enough text to be extracted by Readability.</p>
</article>
</body>
</html>"#;

    let url = url::Url::parse("https://example.com").unwrap();
    let mock = MockHttpClient::new().with_ok_response(url.as_str(), html);

    let result = scrape_with_readability(&mock, &url).await;
    match &result {
        Ok(contents) => {
            assert!(!contents.is_empty());
            assert!(!contents[0].content.is_empty());
        },
        Err(e) => panic!("mock HTML should succeed, got: {e}"),
    }
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_404_returns_http_error() {
    let url = url::Url::parse("https://example.com/notfound").unwrap();
    let mock = MockHttpClient::new().with_response(url.as_str(), Err(HttpError::ClientError(404)));

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        matches!(err, ScraperError::Http { status: 404, .. }),
        "expected Http(404), got: {err}"
    );
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_empty_body_graceful_handling() {
    let url = url::Url::parse("https://example.com").unwrap();
    let mock = MockHttpClient::new().with_ok_response(url.as_str(), "");

    let result = scrape_with_readability(&mock, &url).await;
    // Empty body should not panic — Readability or fallback handles it
    match &result {
        Ok(contents) => assert!(!contents.is_empty()),
        Err(e) => panic!("empty body should succeed, got: {e}"),
    }
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_timeout_error_propagation() {
    let url = url::Url::parse("https://slow.example.com").unwrap();
    let mock = MockHttpClient::new().with_response(url.as_str(), Err(HttpError::Timeout));

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("timeout"),
        "error should mention timeout: {msg}"
    );
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_connection_error_propagation() {
    let url = url::Url::parse("https://unreachable.example.com").unwrap();
    let mock = MockHttpClient::new().with_response(
        url.as_str(),
        Err(HttpError::Connection("connection refused".into())),
    );

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());

    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("connection refused"),
        "error should mention connection: {msg}"
    );
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_forbidden_returns_403() {
    let url = url::Url::parse("https://blocked.example.com").unwrap();
    let mock = MockHttpClient::new().with_response(url.as_str(), Err(HttpError::Forbidden));

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        ScraperError::Http { status, .. } => assert_eq!(status, 403),
        other => panic!("expected Http(403), got: {other}"),
    }
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_server_error_returns_500() {
    let url = url::Url::parse("https://error.example.com").unwrap();
    let mock = MockHttpClient::new().with_response(url.as_str(), Err(HttpError::ServerError(500)));

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        ScraperError::Http { status, .. } => assert_eq!(status, 500),
        other => panic!("expected Http(500), got: {other}"),
    }
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_non_200_status_returns_error() {
    let url = url::Url::parse("https://example.com").unwrap();
    let mock = MockHttpClient::new().with_response(
        url.as_str(),
        Ok(HttpResponse {
            status: 301,
            body: String::new(),
            headers: HashMap::new(),
        }),
    );

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_rate_limited_error() {
    let url = url::Url::parse("https://api.example.com").unwrap();
    let mock = MockHttpClient::new().with_response(url.as_str(), Err(HttpError::RateLimited(60)));

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());

    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("rate limited"),
        "error should mention rate limiting: {msg}"
    );
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_waf_challenge_error() {
    let url = url::Url::parse("https://protected.example.com").unwrap();
    let mock = MockHttpClient::new().with_response(
        url.as_str(),
        Err(HttpError::WafChallenge("Cloudflare".into())),
    );

    let result = scrape_with_readability(&mock, &url).await;
    assert!(result.is_err());

    match result.unwrap_err() {
        ScraperError::WafBlocked { provider, .. } => {
            assert_eq!(provider, "Cloudflare");
        },
        other => panic!("expected WafBlocked, got: {other}"),
    }
}

// =====================================================================
// Mutation-killing tests for scraper_service
// =====================================================================

#[cfg_attr(miri, ignore)] // lol_html/servo_arc Tree-Borrows UB via clean_html
#[tokio::test]
async fn test_scrape_multiple_with_limit_returns_results() {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body>
<article>
<h1>Article Title</h1>
<p>This is substantial content that should be extracted by Readability. It has enough text to pass the minimum threshold.</p>
</article>
</body>
</html>"#;

    let url1 = url::Url::parse("https://example.com/page1").unwrap();
    let url2 = url::Url::parse("https://example.com/page2").unwrap();
    let mock = MockHttpClient::new()
        .with_ok_response(url1.as_str(), html)
        .with_ok_response(url2.as_str(), html);

    let config = ScraperConfig::default();
    let result = scrape_multiple_with_limit(&mock, &[url1, url2], &config, None)
        .await
        .expect("scrape_multiple_with_limit should succeed");

    assert_eq!(result.len(), 2, "should return content from both URLs");
}

#[tokio::test]
async fn test_scrape_multiple_with_limit_empty_urls() {
    let mock = MockHttpClient::new();
    let config = ScraperConfig::default();
    let result = scrape_multiple_with_limit(&mock, &[], &config, None)
        .await
        .expect("empty URL list should return Ok");
    assert!(result.is_empty());
}

#[test]
fn test_download_assets_disabled_returns_empty() {
    let config = ScraperConfig::default();
    assert!(!config.has_downloads());
}

#[test]
fn test_download_assets_enabled_config() {
    let config = ScraperConfig::default().with_images();
    assert!(config.has_downloads());
}

#[test]
fn test_max_instrumented_body_size_is_1mb() {
    assert_eq!(MAX_INSTRUMENTED_BODY_SIZE, 1_048_576);
}

#[test]
fn test_min_content_chars_is_50() {
    assert_eq!(MIN_CONTENT_CHARS, 50);
}

// =====================================================================
// extract_with_selector tests (pure function, no I/O)
// =====================================================================

#[test]
fn test_extract_with_selector_body_passthrough() {
    let html = "<html><body><p>Hello</p></body></html>";
    let result = extract_with_selector(html, "body", None);
    assert!(result.is_matched(), "selector 'body' should return Matched");
    assert_eq!(
        result.as_html(),
        html,
        "selector 'body' should return original HTML unchanged"
    );
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_extracts_matching_elements() {
    let html = r#"<html><body>
            <div class="main"><p>Main content</p></div>
            <div class="sidebar"><p>Sidebar</p></div>
        </body></html>"#;
    let result = extract_with_selector(html, "div.main", None);
    assert!(result.is_matched(), "should return Matched");
    assert!(
        result.as_html().contains("Main content"),
        "should contain matched element content"
    );
    assert!(
        result.as_html().contains("selector-extracted"),
        "should wrap in selector-extracted div"
    );
    assert!(
        !result.as_html().contains("Sidebar"),
        "should NOT contain unmatched element content"
    );
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_no_matches_falls_back() {
    let html = "<html><body><p>Hello</p></body></html>";
    let result = extract_with_selector(html, "article", None);
    assert!(!result.is_matched(), "no matches should return Fallback");
    assert_eq!(
        result.as_html(),
        html,
        "no matches should fall back to original HTML"
    );
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_invalid_syntax_falls_back() {
    let html = "<html><body><p>Hello</p></body></html>";
    let result = extract_with_selector(html, ">>>invalid", None);
    assert!(
        !result.is_matched(),
        "invalid selector syntax should return Fallback"
    );
    assert_eq!(
        result.as_html(),
        html,
        "invalid selector syntax should fall back to original HTML"
    );
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_multiple_matches_joined() {
    let html = r#"<html><body>
            <li>Item 1</li>
            <li>Item 2</li>
            <li>Item 3</li>
        </body></html>"#;
    let result = extract_with_selector(html, "li", None);
    assert!(result.is_matched(), "should return Matched");
    assert!(result.as_html().contains("Item 1"));
    assert!(result.as_html().contains("Item 2"));
    assert!(result.as_html().contains("Item 3"));
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_id_selector() {
    let html = r#"<html><body>
            <div id="target"><p>Targeted</p></div>
            <div id="other"><p>Other</p></div>
        </body></html>"#;
    let result = extract_with_selector(html, "#target", None);
    assert!(result.is_matched(), "should return Matched");
    assert!(result.as_html().contains("Targeted"));
    assert!(!result.as_html().contains("Other"));
}

// ---------------------------------------------------------------------
// ExtractResult diagnostic behavior with/without inspector
// ---------------------------------------------------------------------

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_matched() {
    let html = r#"<html><body>
            <article><p>Article content</p></article>
        </body></html>"#;
    let result = extract_with_selector(html, "article", None);
    assert!(
        result.is_matched(),
        "valid selector with matches should return Matched"
    );
    assert!(
        result.as_html().contains("Article content"),
        "matched HTML should contain the article content"
    );
    assert!(
        result.as_html().contains("selector-extracted"),
        "matched HTML should be wrapped in selector-extracted div"
    );
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_zero_matches_with_inspector() {
    use webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector;

    let html = r#"<html><body>
            <div class="main"><p>Main content</p></div>
        </body></html>"#;
    let inspector = DefaultDomInspector::new();
    let result = extract_with_selector(
        html,
        ".nonexistent",
        Some(&inspector as &dyn DomInspectorPort),
    );
    assert!(!result.is_matched(), "zero matches should return Fallback");
    match &result {
        ExtractResult::Fallback {
            html: _,
            diagnostic,
        } => {
            let diag = diagnostic
                .as_ref()
                .expect("diagnostic should be Some when inspector is provided");
            assert_eq!(
                diag.error_kind,
                SelectorErrorKind::ZeroMatches,
                "error kind should be ZeroMatches"
            );
            assert!(
                !diag.report.tag_counts.is_empty(),
                "report should contain tag counts from the DOM"
            );
        },
        ExtractResult::Matched(_) => panic!("expected Fallback, got Matched"),
    }
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_zero_matches_no_inspector() {
    let html = "<html><body><p>Hello</p></body></html>";
    let result = extract_with_selector(html, "article", None);
    assert!(!result.is_matched(), "zero matches should return Fallback");
    match &result {
        ExtractResult::Fallback {
            html: _,
            diagnostic,
        } => {
            assert!(
                diagnostic.is_none(),
                "diagnostic should be None when no inspector is provided"
            );
        },
        ExtractResult::Matched(_) => panic!("expected Fallback, got Matched"),
    }
}

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_invalid_syntax() {
    use webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector;

    let html = "<html><body><p>Hello</p></body></html>";
    let inspector = DefaultDomInspector::new();
    let result =
        extract_with_selector(html, "div >>> p", Some(&inspector as &dyn DomInspectorPort));
    assert!(
        !result.is_matched(),
        "invalid selector syntax should return Fallback"
    );
    match &result {
        ExtractResult::Fallback {
            html: _,
            diagnostic,
        } => {
            let diag = diagnostic
                .as_ref()
                .expect("diagnostic should be Some when inspector is provided");
            assert!(
                matches!(&diag.error_kind, SelectorErrorKind::InvalidSelector(_)),
                "error kind should be InvalidSelector"
            );
        },
        ExtractResult::Matched(_) => panic!("expected Fallback, got Matched"),
    }
}

// =====================================================================
// Empty HTML document -> EmptyDocument diagnostic
// =====================================================================

#[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
#[test]
fn test_extract_with_selector_empty_html() {
    use webfang_core::infrastructure::scraper::dom_inspector::DefaultDomInspector;

    let inspector = DefaultDomInspector::new();

    // --- Truly empty HTML ---
    let result = extract_with_selector("", "article", Some(&inspector as &dyn DomInspectorPort));
    assert!(
        !result.is_matched(),
        "empty HTML should return Fallback, not Matched"
    );
    match &result {
        ExtractResult::Fallback { html, diagnostic } => {
            assert!(
                html.is_empty(),
                "fallback HTML should be the original empty string"
            );
            let diag = diagnostic
                .as_ref()
                .expect("diagnostic should be Some when inspector is provided");
            assert_eq!(
                diag.error_kind,
                SelectorErrorKind::EmptyDocument,
                "empty HTML should produce EmptyDocument, not ZeroMatches"
            );
        },
        ExtractResult::Matched(_) => panic!("expected Fallback, got Matched"),
    }

    // --- Whitespace-only HTML ---
    let result = extract_with_selector(
        "   \n\t  ",
        "article",
        Some(&inspector as &dyn DomInspectorPort),
    );
    assert!(
        !result.is_matched(),
        "whitespace-only HTML should return Fallback"
    );
    match &result {
        ExtractResult::Fallback { diagnostic, .. } => {
            let diag = diagnostic
                .as_ref()
                .expect("diagnostic should be Some when inspector is provided");
            assert_eq!(
                diag.error_kind,
                SelectorErrorKind::EmptyDocument,
                "whitespace-only HTML should produce EmptyDocument"
            );
        },
        ExtractResult::Matched(_) => panic!("expected Fallback, got Matched"),
    }

    // --- Empty HTML without inspector -> diagnostic is None ---
    let result = extract_with_selector("", "article", None);
    assert!(
        !result.is_matched(),
        "empty HTML without inspector should still return Fallback"
    );
    match &result {
        ExtractResult::Fallback { html, diagnostic } => {
            assert!(html.is_empty(), "fallback HTML should be empty");
            assert!(
                diagnostic.is_none(),
                "diagnostic should be None when no inspector is provided"
            );
        },
        ExtractResult::Matched(_) => panic!("expected Fallback, got Matched"),
    }

    // --- Empty HTML with body selector -> backward compat (returns Matched) ---
    let result = extract_with_selector("", "body", None);
    assert!(
        result.is_matched(),
        "body selector on empty HTML should return Matched (backward compat)"
    );
    assert_eq!(
        result.as_html(),
        "",
        "body selector on empty HTML should return the empty string unchanged"
    );
}

// =====================================================================
// scrape_multiple_with_limit partial failure
// =====================================================================

#[cfg_attr(miri, ignore)] // lol_html/servo_arc Tree-Borrows UB via clean_html
#[tokio::test]
async fn test_scrape_multiple_partial_failure() {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body>
<article>
<h1>Article Title</h1>
<p>This is substantial content that should be extracted by Readability. It has enough text to pass the minimum threshold.</p>
</article>
</body>
</html>"#;

    let url_ok = url::Url::parse("https://example.com/ok").unwrap();
    let url_fail = url::Url::parse("https://example.com/fail").unwrap();
    let mock = MockHttpClient::new()
        .with_ok_response(url_ok.as_str(), html)
        .with_response(url_fail.as_str(), Err(HttpError::ClientError(404)));

    let config = ScraperConfig::default();
    let result = scrape_multiple_with_limit(&mock, &[url_ok, url_fail], &config, None)
        .await
        .expect("should not fail overall even with partial URL failures");

    assert_eq!(
        result.len(),
        1,
        "only the successful URL should produce content"
    );
}

// =====================================================================
// Title extraction verification
// =====================================================================

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_extracts_title() {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>My Page Title</title></head>
<body>
<article>
<h1>Main Heading</h1>
<p>This is enough content to pass the minimum character threshold for readability extraction algorithm to work properly.</p>
</article>
</body>
</html>"#;

    let url = url::Url::parse("https://example.com").unwrap();
    let mock = MockHttpClient::new().with_ok_response(url.as_str(), html);

    let result = scrape_with_readability(&mock, &url).await.unwrap();
    assert!(!result.is_empty());
    // Readability should extract the title
    assert!(
        !result[0].title.is_empty(),
        "title should be extracted from HTML"
    );
}

#[cfg_attr(miri, ignore)] // legible/servo_arc Tree-Borrows UB
#[tokio::test]
async fn test_mock_extracts_non_empty_content() {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Page</title></head>
<body>
<article>
<h1>Heading</h1>
<p>Lorem ipsum dolor sit amet, consectetur adipiscing elit. Sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam.</p>
</article>
</body>
</html>"#;

    let url = url::Url::parse("https://example.com").unwrap();
    let mock = MockHttpClient::new().with_ok_response(url.as_str(), html);

    let result = scrape_with_readability(&mock, &url).await.unwrap();
    assert!(!result.is_empty());
    assert!(
        !result[0].content.is_empty(),
        "content should be non-empty after extraction"
    );
    assert_eq!(result[0].url.as_str(), url.as_str());
}

// =====================================================================
// Spy test: prove download_assets_if_enabled reuses shared Arc<Downloader>
// =====================================================================

/// Spy test: prove `download_assets_if_enabled` reuses the injected
/// `AssetDownloaderPort` instance (Issue #217).
///
/// Uses a `DownloadSpy` mock that counts `download_batch` calls.
/// If the function constructed a new Downloader instead of using the injected
/// mock, the call count would be 0 — proving the trait object is passed through.
#[tokio::test]
async fn test_download_assets_shared_downloader_spy() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use webfang_core::domain::entities::DownloadedAsset as DomainDownloadedAsset;
    use webfang_core::domain::ports::AssetDownloaderPort;

    /// Spy that counts download_batch calls without doing real I/O.
    struct DownloadSpy {
        call_count: AtomicUsize,
        last_urls: Mutex<Vec<String>>,
    }

    impl DownloadSpy {
        fn new() -> Self {
            Self {
                call_count: AtomicUsize::new(0),
                last_urls: Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }

        fn last_urls(&self) -> Vec<String> {
            self.last_urls.lock().unwrap().clone()
        }
    }

    impl AssetDownloaderPort for DownloadSpy {
        fn download_batch(
            &self,
            urls: &[String],
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = webfang_core::error::Result<Vec<DomainDownloadedAsset>>,
                    > + Send
                    + '_,
            >,
        > {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_urls.lock().unwrap() = urls.to_vec();
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let config = ScraperConfig::default()
        .with_images()
        .with_output_dir(tmp.path().to_path_buf());

    let spy = DownloadSpy::new();

    let html = r#"<!DOCTYPE html>
<html>
<body>
<img src="https://httpbin.org/image.png" alt="test1">
<img src="https://httpbin.org/image2.png" alt="test2">
</body>
</html>"#;
    let base = url::Url::parse("https://example.com/page").unwrap();

    let result = webfang_core::application::scraper_service::download_assets_if_enabled(
        html,
        &base,
        &config,
        Some(&spy),
    )
    .await
    .expect("download_assets_if_enabled should succeed");

    // Proof: the injected spy was called — not a new Downloader constructed
    assert_eq!(
        spy.call_count(),
        1,
        "download_batch must be called exactly once"
    );
    assert_eq!(
        spy.last_urls().len(),
        2,
        "must extract 2 image URLs from HTML"
    );
    assert!(result.is_empty(), "spy returns empty vec");
}

/// Regression guard (C3 / REQ-2.1.2): `None` downloader fallback still works.
///
/// When `download_assets_if_enabled` receives `None`, the fallback branch
/// (scraper_service.rs:572-576) constructs a per-call `Downloader::new`.
/// This test proves that path compiles and executes after REQ-2.1.1 changes.
#[tokio::test]
async fn test_download_assets_none_fallback_still_works() {
    let html = r#"<!DOCTYPE html>
<html>
<body>
<img src="https://example.com/img.png" alt="test">
</body>
</html>"#;
    let base = url::Url::parse("https://example.com/page").unwrap();
    let config = ScraperConfig::default().with_images();

    // None triggers the fallback Downloader::new path.
    // With unreachable URLs, downloads fail gracefully -> empty result.
    let result = webfang_core::application::scraper_service::download_assets_if_enabled(
        html, &base, &config, None,
    )
    .await
    .expect("None fallback should not fail");

    // Downloads are enabled but URLs unreachable -> empty (graceful failure).
    assert!(
        result.is_empty(),
        "unreachable URLs should produce empty assets via fallback"
    );
}

/// Triangulation: downloads-disabled path returns empty immediately,
/// regardless of downloader argument.
#[tokio::test]
async fn test_download_assets_disabled_returns_empty_immediately() {
    let html = r#"<!DOCTYPE html>
<html>
<body><img src="https://example.com/img.png" alt="test"></body>
</html>"#;
    let base = url::Url::parse("https://example.com/page").unwrap();
    let config = ScraperConfig::default(); // downloads disabled

    let result = webfang_core::application::scraper_service::download_assets_if_enabled(
        html, &base, &config, None,
    )
    .await
    .expect("disabled downloads should succeed");

    assert!(
        result.is_empty(),
        "downloads disabled -> empty result without touching Downloader"
    );
}
