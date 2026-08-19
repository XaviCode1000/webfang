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
        let document = scraper::Html::parse_document(html);
        return fallback(
            html,
            inspector,
            &document,
            SelectorErrorKind::EmptyDocument,
            selector,
            "HTML document is empty or whitespace-only, falling back with EmptyDocument diagnostic",
        );
    }

    let document = scraper::Html::parse_document(html);
    let sel = match scraper::Selector::parse(selector) {
        Ok(s) => s,
        Err(e) => {
            return fallback(
                html,
                inspector,
                &document,
                // The diagnostic keeps the raw crate error for debugging;
                // the user-facing WARN must not leak library jargon (#761).
                SelectorErrorKind::InvalidSelector(e.to_string()),
                selector,
                &invalid_selector_message(selector),
            );
        },
    };

    let matched: Vec<String> = document.select(&sel).map(|el| el.html()).collect();

    if matched.is_empty() {
        return fallback(
            html,
            inspector,
            &document,
            SelectorErrorKind::ZeroMatches,
            selector,
            &format!("CSS selector '{selector}' matched 0 elements, falling back to full HTML"),
        );
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

/// Emit the fallback warning and build a [`ExtractResult::Fallback`] carrying
/// the full HTML and a diagnostic for the given [`SelectorErrorKind`], when an
/// inspector is available.
/// Clean user-facing message for an invalid CSS selector (#761).
///
/// The `selectors` crate's error text leaks library jargon
/// (`NoQualifiedNameInAttributeSelector(...)`, "Please report this to the
/// developer") into production stderr. The raw error is preserved in the
/// [`SelectorErrorKind::InvalidSelector`] diagnostic for debugging; only this
/// sanitized message reaches the user.
fn invalid_selector_message(selector: &str) -> String {
    format!("selector CSS inválido: '{selector}' — se usó el HTML completo como fallback")
}

fn fallback(
    html: &str,
    inspector: Option<&dyn DomInspectorPort>,
    document: &scraper::Html,
    kind: SelectorErrorKind,
    selector: &str,
    message: &str,
) -> ExtractResult {
    warn!("{}", message);
    ExtractResult::Fallback {
        html: html.to_owned(),
        diagnostic: build_diagnostic(inspector, document, kind, selector),
    }
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
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    // Standalone convenience entry: this call IS the operation, so it mints
    // its own run-root identity (#501). No robots fetcher is available at
    // this standalone entry point (#697): enforcement is the caller's
    // concern (the MCP/CLI paths wire their own robots handling).
    let root_correlation = CorrelationId::new();
    let outcome = scrape_with_config(
        client,
        url,
        &ScraperConfig::default(),
        None,
        None,
        None,
        None,
        false,
        &root_correlation,
    )
    .await?;
    Ok(outcome.results)
}

/// Canonical adaptive selector repair (Tier 1 lexical cascade).
///
/// Single home for the "extraction fell back → ask the adaptive engine for a
/// repaired selector → re-extract" use case, shared by [`extract_content`] and
/// `scrape_with_config` (issue #442 — previously duplicated in both). When the
/// engine yields a selector that matches, the repaired [`ExtractResult`] is
/// returned; otherwise the original fallback is preserved unchanged.
///
/// `inspector` is threaded through to [`extract_with_selector`] for diagnostics
/// (`scrape_with_config` passes its inspector; [`extract_content`] passes
/// `None`).
#[cfg(feature = "adaptive-selectors")]
pub(crate) async fn adaptive_selector_repair(
    extract_result: ExtractResult,
    engine: Option<&AdaptiveSelectorEngine>,
    selector: &str,
    host: Option<&str>,
    inspector: Option<&dyn DomInspectorPort>,
) -> ExtractResult {
    if let ExtractResult::Fallback { html, diagnostic } = extract_result {
        if let Some(engine) = engine {
            match engine
                .select_sync_aware(
                    html.clone(),
                    selector.to_owned(),
                    host.map(|s| s.to_owned()),
                )
                .await
            {
                Ok(outcome) => {
                    let repaired =
                        extract_with_selector(&html, &outcome.suggestion.selector, inspector);
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
    }
}

/// Pipeline de extracción de contenido: clean → selector → adaptive → readability/fallback.
///
/// Recibe HTML ya fetchado y validado (post-WAF). No conoce el transporte.
///
/// Correlation identity (#501): the per-page identity is a REQUIRED input —
/// callers own it (e.g. `scrape_single_url_for_tui` declares it on its trace
/// span) and inject it here so the exported content shares the same identity
/// as the page's `span_fields` in the `--trace-file` JSONL. Standalone
/// callers mint their own root at entry. Identity enters through the type
/// system or not at all — there is no ad-hoc fallback.
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
    correlation_id: &CorrelationId,
) -> Result<ScrapedContent> {
    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = crate::infrastructure::converter::html_cleaner::clean_html(html);

    // Apply CSS selector extraction if a non-default selector is configured.
    let extract_result = extract_with_selector(&cleaned_html, &config.selector, None);
    // Adaptive selector repair (Tier 1 lexical): delegate to the canonical
    // shared helper (#442). This TUI path passes no inspector and keeps its own
    // binary/metrics handling instead of delegating to `scrape_with_config`.
    #[cfg(feature = "adaptive-selectors")]
    let extract_result = adaptive_selector_repair(
        extract_result,
        engine,
        &config.selector,
        url.host_str(),
        None,
    )
    .await;

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

            // Shared minimum-content guard (#706): on the SUCCESS branch there
            // was no content-size check before, so JS-shell pages returned Ok
            // near-empty. The fallback branch keeps MIN_FALLBACK_CONTENT=100 as
            // its SOLE authority — no second guard there (no double-error).
            crate::application::spa_detection::validate_min_content(
                url.as_str(),
                &article.text_content,
                html,
                correlation_id,
            )?;

            let author = crate::infrastructure::scraper::author_extractor::extract_author(
                html,
                article.byline.as_deref(),
            );

            Ok(ScrapedContent {
                title: crate::application::resolve_title(&article.title, url),
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt.as_deref().map(|e| {
                    crate::domain::excerpt_repair::repair_empty_byline(e, author.as_deref())
                }),
                author,
                date: article.published_time,
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                html: Some(article.content),
                assets,
                correlation_id: Some(correlation_id.clone()),
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
                    Some(correlation_id),
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
                correlation_id: Some(correlation_id.clone()),
            })
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Miri gating policy for this module (#764, #487): 6 tests below trip the
    // known servo_arc 0.4.3 ArcUnion Tree Borrows UB via two entry points:
    // - valid-selector tests drop a `scraper::Selector` built in
    //   `extract_with_selector` (same defect family as dom_inspector, #763);
    // - `scrape_with_readability`/`extract_content` tests reach `clean_html`
    //   (html_cleaner.rs:112) → `lol_html::rewrite_str`, whose drop glue ends
    //   in the same `servo_arc::ArcUnion` drop (recurrence of #498, same
    //   class as the discovery.rs gates in #486).
    // The 3 ungated tests avoid it: body passthrough (no Selector), and the
    // invalid-selector / whitespace-only fallbacks (parse failure happens
    // before `selectors::SelectorList` is constructed, so nothing drops).
    // Regular (non-Miri) runs keep full coverage.

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

    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB: drops parsed Selector (#487, #764)
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

    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB: drops parsed Selector (#487, #764)
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

    /// #761: the user-facing WARN must not leak `selectors` crate jargon
    /// ("Please report this to the developer", variant debug names).
    #[test]
    fn test_invalid_selector_message_is_clean() {
        let msg = super::invalid_selector_message("[[[invalid");
        assert!(
            msg.contains("'[[[invalid'"),
            "must name the offending selector"
        );
        assert!(
            !msg.contains("Please report this to the developer"),
            "must not leak crate jargon: {msg}"
        );
        assert!(
            !msg.contains("NoQualifiedNameInAttributeSelector"),
            "must not leak variant debug names: {msg}"
        );
    }

    #[test]
    fn test_empty_html_falls_back() {
        let result = extract_with_selector("   ", "div.main", None);
        assert!(!result.is_matched(), "whitespace-only HTML must fall back");
    }

    // --- scrape_with_readability: ephemeral mock HTTP client, no real network ---

    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
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

    // --- extract_content: the shared minimum-content guard (#706) ---

    /// CLI funnel success branch (#684): readability succeeds but yields
    /// sub-threshold text on a JS-shell page → typed `ExtractionFailed` with
    /// the marker Spanish reason, never Ok near-empty.
    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
    #[tokio::test]
    async fn test_extract_content_success_branch_below_threshold_errors() {
        let html = "<html><head><title>App</title></head><body>\
             <article><h1>Status</h1><p>Initializing…</p></article>\
             <div id=\"root\"></div>\
             </body></html>";
        let url = url::Url::parse("https://spa.example.com/app").unwrap();
        let corr = CorrelationId::new();

        let result = extract_content(html, &url, &ScraperConfig::default(), None, None, &corr)
            .await
            .expect_err("sub-threshold success-branch content must fail honestly");

        match &result {
            ScraperError::ExtractionFailed {
                url: failed_url,
                reason,
            } => {
                assert_eq!(failed_url, url.as_str(), "url must be preserved");
                assert!(
                    reason.contains("contenido insuficiente"),
                    "Spanish reason must state insufficient content: {reason}"
                );
                assert!(
                    reason.contains("renderizado de JavaScript"),
                    "marker-bearing shell must report the JS cause: {reason}"
                );
            },
            other => panic!("expected ExtractionFailed, got: {other}"),
        }
    }

    /// XC-2 legit fixture: a server-rendered page with ≥50 chars of extractable
    /// text MUST still succeed — the guard never fires above the threshold.
    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
    #[tokio::test]
    async fn test_extract_content_legit_page_above_threshold_succeeds() {
        let html = "<html><head><title>Docs</title></head><body><article>\
             <h1>Guide</h1>\
             <p>This is a substantially long paragraph of server-rendered \
             article content, easily over the fifty character extraction \
             threshold, so the guard must let it through untouched.</p>\
             </article></body></html>";
        let url = url::Url::parse("https://example.com/guide").unwrap();
        let corr = CorrelationId::new();

        let result = extract_content(html, &url, &ScraperConfig::default(), None, None, &corr)
            .await
            .expect("a legit page must keep scraping successfully");

        assert!(
            result.content.chars().count() >= crate::application::spa_detection::MIN_CONTENT_CHARS,
            "legit pages must yield substantial content"
        );
    }

    /// CE-3 no double-gate: when readability FAILS, `MIN_FALLBACK_CONTENT=100`
    /// stays the SOLE authority on the fallback branch — the 50-char guard must
    /// NOT fire there (its Spanish text would be a duplicate signal).
    #[cfg_attr(miri, ignore)] // servo_arc 0.4.3 Tree Borrows UB in ArcUnion drop via lol_html (#487, #764)
    #[tokio::test]
    async fn test_extract_content_poor_fallback_keeps_min_fallback_authority() {
        let html = "<html><body><a href=\"/x\"></a></body></html>";
        let url = url::Url::parse("https://example.com/empty").unwrap();
        let corr = CorrelationId::new();

        let result = extract_content(html, &url, &ScraperConfig::default(), None, None, &corr)
            .await
            .expect_err("a content-less page must fail");

        match &result {
            ScraperError::ExtractionFailed { reason, .. } => {
                assert!(
                    reason.contains("contenido pobre del fallback"),
                    "fallback branch error must come from MIN_FALLBACK_CONTENT, got: {reason}"
                );
                assert!(
                    !reason.contains("contenido insuficiente"),
                    "the 50-char guard must NOT double-fire on the fallback branch: {reason}"
                );
            },
            other => panic!("expected ExtractionFailed, got: {other}"),
        }
    }
}
