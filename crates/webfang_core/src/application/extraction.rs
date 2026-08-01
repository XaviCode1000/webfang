//! Content extraction — CSS selector extraction and Readability entry point.
//!
//! Hosts the pure CSS-selector extraction logic and the thin
//! [`scrape_with_readability`] convenience wrapper over the orchestrating
//! `scrape_with_config` use case.

use crate::application::diagnostic::build_diagnostic;
use crate::application::http_client::HttpClientPort;
use crate::application::scraper_service::scrape_with_config;
use crate::domain::{
    CorrelationId, DomInspectorPort, ExtractResult, ScrapedContent, SelectorErrorKind, ValidUrl,
};
use crate::error::{Result, ScraperError};
use crate::infrastructure::observability::log_scrape_error;
use crate::infrastructure::scraper::{fallback, readability};
use crate::ScraperConfig;
use tracing::{debug, warn};

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

/// Extract HTML content using a CSS selector.
///
/// When `selector` is not "body", parses the HTML and extracts all elements
/// matching the selector. Returns the outer HTML of matched elements wrapped
/// in a `<div>` for Readability processing. If no elements match or the
/// selector is invalid, returns [`ExtractResult::Fallback`] with the full
/// HTML and an optional diagnostic (when an inspector is provided).
///
/// # Arguments
/// * `html` - The HTML document to extract from
/// * `selector` - CSS selector string (use `"body"` to skip extraction)
/// * `inspector` - Optional DOM inspector for diagnostics on failure paths
pub fn extract_with_selector(
    html: &str,
    selector: &str,
    inspector: Option<&dyn DomInspectorPort>,
) -> ExtractResult {
    if selector == "body" {
        return ExtractResult::Matched(html.to_owned());
    }

    // Early check: empty or whitespace-only HTML. `scraper::Html::parse_document("")`
    // creates 3 implicit elements (html, head, body), so without this check the
    // selector matching would fall through to ZeroMatches instead of
    // EmptyDocument — leaving SelectorErrorKind::EmptyDocument as dead code.
    if html.trim().is_empty() {
        warn!(
            "HTML document is empty or whitespace-only, falling back with EmptyDocument diagnostic"
        );
        let document = scraper::Html::parse_document(html);
        return ExtractResult::Fallback {
            html: html.to_owned(),
            diagnostic: build_diagnostic(
                inspector,
                &document,
                SelectorErrorKind::EmptyDocument,
                selector,
            ),
        };
    }

    let document = scraper::Html::parse_document(html);
    let sel = match scraper::Selector::parse(selector) {
        Ok(s) => s,
        Err(e) => {
            warn!(
                "Invalid CSS selector '{}': {}, falling back to full HTML",
                selector, e
            );
            return ExtractResult::Fallback {
                html: html.to_owned(),
                diagnostic: build_diagnostic(
                    inspector,
                    &document,
                    SelectorErrorKind::InvalidSelector(e.to_string()),
                    selector,
                ),
            };
        },
    };

    let matched: Vec<String> = document.select(&sel).map(|el| el.html()).collect();

    if matched.is_empty() {
        warn!(
            "CSS selector '{}' matched 0 elements, falling back to full HTML",
            selector
        );
        return ExtractResult::Fallback {
            html: html.to_owned(),
            diagnostic: build_diagnostic(
                inspector,
                &document,
                SelectorErrorKind::ZeroMatches,
                selector,
            ),
        };
    }

    debug!(
        "CSS selector '{}' matched {} elements",
        selector,
        matched.len()
    );

    ExtractResult::Matched(format!(
        "<div id=\"selector-extracted\">{}</div>",
        matched.join("\n")
    ))
}

/// Scrape a URL using Readability algorithm for clean content extraction
///
/// This is the 2026 best practice approach — uses the same algorithm as
/// Firefox Reader View to extract only meaningful content.
///
/// # Examples
///
/// ```no_run
/// use webfang_core::application::{create_http_client, scrape_with_readability};
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let client = create_http_client()?;
/// let url = url::Url::parse("https://example.com")?;
/// let results = scrape_with_readability(&client, &url).await?;
/// # Ok(())
/// # }
/// ```
pub async fn scrape_with_readability(
    client: &dyn HttpClientPort,
    url: &url::Url,
) -> Result<Vec<ScrapedContent>> {
    let outcome =
        scrape_with_config(client, url, &ScraperConfig::default(), None, None, None).await?;
    Ok(outcome.results)
}

/// Pipeline de extracción de contenido: clean → selector → adaptive → readability/fallback.
///
/// Recibe HTML ya fetchado y validado (post-WAF). No conoce el transporte.
///
/// # Errors
///
/// Returns [`ScraperError::ExtractionFailed`] when fallback content is below
/// `MIN_FALLBACK_CONTENT` bytes.
pub async fn extract_content(
    html: &str,
    url: &url::Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
) -> Result<ScrapedContent> {
    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = crate::infrastructure::converter::html_cleaner::clean_html(html);

    // Apply CSS selector extraction if a non-default selector is configured.
    let extract_result = extract_with_selector(&cleaned_html, &config.selector, None);

    // Adaptive selector repair (Tier 1 lexical): when extraction falls back and
    // an engine is wired in, try to find a repaired selector and re-extract.
    // Mirrors the repair in `scrape_with_config`; this TUI path keeps its own
    // binary/metrics handling instead of delegating to that function.
    #[cfg(feature = "adaptive-selectors")]
    let extract_result = if let ExtractResult::Fallback { html, diagnostic } = extract_result {
        if let Some(engine) = engine {
            match engine
                .select_sync_aware(
                    html.clone(),
                    config.selector.clone(),
                    url.host_str().map(|s| s.to_owned()),
                )
                .await
            {
                Ok(outcome) => {
                    let repaired = extract_with_selector(&html, &outcome.suggestion.selector, None);
                    if repaired.is_matched() {
                        tracing::info!(
                            repaired_selector = %outcome.suggestion.selector,
                            method = ?outcome.status,
                            "adaptive_repair_resolved"
                        );
                        repaired
                    } else {
                        ExtractResult::Fallback { html, diagnostic }
                    }
                },
                Err(_) => ExtractResult::Fallback { html, diagnostic },
            }
        } else {
            ExtractResult::Fallback { html, diagnostic }
        }
    } else {
        extract_result
    };

    let extraction_html = extract_result.as_html().to_owned();

    // Try Readability first, fallback to plain text extraction
    match readability::parse(&extraction_html, Some(url.as_str())) {
        Ok(article) => {
            let assets = crate::application::asset_download::download_assets_if_enabled(
                html,
                url,
                config,
                asset_downloader,
            )
            .await?;

            Ok(ScrapedContent {
                title: crate::application::resolve_title(&article.title, url),
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                html: Some(article.content),
                assets,
                correlation_id: Some(CorrelationId::new()),
            })
        },
        Err(e) => {
            warn!("Readability failed for {}: {}", url, e);
            let fallback_content = fallback::extract_text(&extraction_html);

            // Check if fallback produced poor content (likely extraction failure)
            const MIN_FALLBACK_CONTENT: usize = 100;
            if fallback_content.len() < MIN_FALLBACK_CONTENT {
                let msg = format!(
                    "contenido pobre del fallback: {} bytes (mín {} bytes). Readability: {}",
                    fallback_content.len(),
                    MIN_FALLBACK_CONTENT,
                    e
                );
                log_scrape_error(
                    &msg,
                    url.as_str(),
                    "extract",
                    None,
                    "content extraction failed",
                );
                return Err(ScraperError::ExtractionFailed {
                    url: url.to_string(),
                    reason: msg,
                });
            }

            let assets = crate::application::asset_download::download_assets_if_enabled(
                html,
                url,
                config,
                asset_downloader,
            )
            .await?;

            Ok(ScrapedContent {
                title: url
                    .host_str()
                    .ok_or_else(|| ScraperError::invalid_url(format!("URL missing host: {url}")))?
                    .to_string(),
                content: fallback_content,
                url: ValidUrl::new(url.clone()),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html.to_owned()),
                assets,
                correlation_id: Some(CorrelationId::new()),
            })
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_with_selector: pure, no network (scraper::Html from a string) ---

    #[test]
    fn test_body_selector_is_passthrough() {
        let html = "<html><body><p>keep me</p></body></html>";
        let result = extract_with_selector(html, "body", None);
        assert!(result.is_matched());
        assert_eq!(
            result.as_html(),
            html,
            "body selector must return HTML verbatim"
        );
    }

    #[test]
    fn test_matching_selector_wraps_elements() {
        let html = "<html><body><div class=\"main\">Hello</div><aside>noise</aside></body></html>";
        let result = extract_with_selector(html, "div.main", None);
        assert!(result.is_matched());
        let extracted = result.as_html();
        assert!(
            extracted.starts_with("<div id=\"selector-extracted\">"),
            "matched output must be wrapped for Readability, got: {extracted}"
        );
        assert!(extracted.contains("Hello"));
        assert!(
            !extracted.contains("noise"),
            "non-matching elements must be excluded"
        );
    }

    #[test]
    fn test_zero_matches_falls_back_without_inspector() {
        let html = "<html><body><p>content</p></body></html>";
        let result = extract_with_selector(html, "div.does-not-exist", None);
        assert!(!result.is_matched());
        assert_eq!(result.as_html(), html, "fallback must carry the full HTML");
        match result {
            ExtractResult::Fallback { diagnostic, .. } => {
                assert!(diagnostic.is_none(), "no inspector means no diagnostic");
            },
            ExtractResult::Matched(_) => panic!("expected Fallback"),
        }
    }

    #[test]
    fn test_invalid_selector_falls_back() {
        let html = "<html><body><p>content</p></body></html>";
        let result = extract_with_selector(html, ">>>not-a-selector", None);
        assert!(!result.is_matched());
        assert_eq!(result.as_html(), html);
    }

    #[test]
    fn test_empty_html_falls_back() {
        let result = extract_with_selector("   ", "div.main", None);
        assert!(!result.is_matched(), "whitespace-only HTML must fall back");
    }

    // --- scrape_with_readability: ephemeral mock HTTP client, no real network ---

    #[tokio::test]
    async fn test_scrape_with_readability_produces_single_result() {
        use crate::domain::http_port::HttpResponse;
        use crate::test_fixtures::MockHttpClient;
        use std::collections::HashMap;

        let url = url::Url::parse("https://example.com").unwrap();
        let article = "<html><head><title>Test Article</title></head><body><article>\
             <h1>Test Article</h1>\
             <p>This is a reasonably long paragraph of article content that should survive \
             readability extraction without any problems at all, providing plenty of text.</p>\
             </article></body></html>";
        let mock = MockHttpClient::new().with_response(
            url.as_str(),
            Ok(HttpResponse {
                status: 200,
                body: article.to_string(),
                headers: HashMap::new(),
            }),
        );

        let results = scrape_with_readability(&mock, &url)
            .await
            .expect("a 200 article response must scrape successfully");

        assert_eq!(results.len(), 1, "one URL must yield exactly one result");
        assert_eq!(
            results[0].url.as_str(),
            url.as_str(),
            "URL must be preserved"
        );
        assert!(
            !results[0].title.is_empty(),
            "title must resolve to non-empty"
        );
    }
}
