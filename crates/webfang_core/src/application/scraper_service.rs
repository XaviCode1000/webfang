//! Scraper service — Main orchestration use case
//!
//! This module coordinates the scraping workflow:
//! 1. Fetch HTML via HTTP client
//! 2. Extract content using Readability or fallback
//! 3. Download assets if configured
//! 4. Return structured ScrapedContent
//!
//! # Rules Applied
//!
//! - **config-externalize**: Concurrency is configurable via ScraperConfig
//! - **async-concurrency-limit**: Uses buffer_unordered for concurrency control

use crate::application::error_mapping::scraper_error_from_http;
use crate::application::http_client::HttpClientPort;
use crate::domain::{CorrelationId, DomInspectorPort, ExtractResult, ScrapedContent, ValidUrl};
use crate::error::{Result, ScraperError};
use crate::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
use crate::infrastructure::observability::log_scrape_error;
use crate::ScraperConfig;
use futures::stream::{self, StreamExt};
use tracing::{debug, info, instrument, warn};

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;
#[cfg(feature = "adaptive-selectors")]
use crate::application::extraction::adaptive_selector_repair;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

// Re-exports preserve the historical `scraper_service::*` public paths after
// the #443 decomposition into focused application modules. Callers (MCP
// handlers, `crawler::discovery`, integration tests) keep resolving unchanged.
pub use crate::application::asset_download::download_assets_if_enabled;
pub use crate::application::extraction::{extract_with_selector, scrape_with_readability};
pub use crate::application::spa_detection::{
    detect_spa_content, SpaDetectionResult, MIN_CONTENT_CHARS,
};

/// Maximum HTML body size to log/instrument (1MB)
/// Bodies larger than this are skipped to avoid performance issues
pub const MAX_INSTRUMENTED_BODY_SIZE: usize = 1_048_576;

/// Outcome of a scrape operation, including selector extraction metadata.
///
/// Contains both the scraped content results and the [`ExtractResult`] from
/// CSS selector extraction, allowing callers (e.g. the MCP handler) to
/// inspect whether the selector matched and access diagnostics.
#[derive(Debug)]
pub struct ScrapeOutcome {
    /// Scraped content results.
    pub results: Vec<ScrapedContent>,
    /// CSS selector extraction result (`Matched` or `Fallback` with optional diagnostic).
    pub extract_result: ExtractResult,
}

impl ScrapeOutcome {
    /// Get the scraped content results as a slice.
    #[must_use]
    pub fn as_results(&self) -> &[ScrapedContent] {
        &self.results
    }
}

/// Scrape a URL with asset downloading configuration
///
/// Correlation contract (#501): the caller owns the run-root identity
/// (`root_correlation`); this use case derives one page child from it
/// (same `trace_id`, fresh `span_id`) so every page of the operation stays
/// reconstructable under a single trace while each page is distinguishable.
///
/// # Arguments
/// * `client` - HTTP client
/// * `url` - URL to scrape
/// * `config` - Scraper configuration with download options
/// * `downloader` - Optional asset downloader
/// * `inspector` - Optional DOM inspector for selector diagnostics (None for non-MCP paths)
/// * `root_correlation` - Run-root correlation identity of the enclosing operation
///
/// # Returns
/// * `ScrapeOutcome` - Scraped content results + CSS selector extraction result
///
/// # Errors
/// Returns `ScraperError::Http` for HTTP errors, `ScraperError::Network` for
/// connection errors.
pub async fn scrape_with_config(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    inspector: Option<&dyn DomInspectorPort>,
    engine: Option<&AdaptiveSelectorEngine>,
    root_correlation: &CorrelationId,
) -> Result<ScrapeOutcome> {
    // Per-page identity derived from the run root BEFORE the span so the span
    // can declare it (`#[instrument]` only sees function parameters — #501).
    let page_correlation = root_correlation.child();
    scrape_with_config_inner(
        client,
        url,
        config,
        downloader,
        inspector,
        engine,
        page_correlation,
    )
    .await
}

#[instrument(
    name = "scrape_with_config",
    skip(client, config, downloader, inspector, engine, correlation),
    fields(
        url = %url,
        correlation_id = %correlation,
        trace_id = %correlation.trace_id(),
        has_downloads = config.has_downloads()
    )
)]
async fn scrape_with_config_inner(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    inspector: Option<&dyn DomInspectorPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    correlation: CorrelationId,
) -> Result<ScrapeOutcome> {
    let mut results = Vec::new();

    info!("🌐 Fetching: {}", url);

    let response = match client.get(url.as_str()).await {
        Ok(resp) => resp,
        Err(e) => {
            log_scrape_error(
                &e,
                url.as_str(),
                "fetch",
                Some(&correlation),
                "HTTP request failed",
            );
            return Err(scraper_error_from_http(e, url.as_str()));
        },
    };

    if !(200..300).contains(&response.status) {
        return Err(ScraperError::http(response.status, url.as_str()));
    }

    let html = response.body;

    // Record HTML size in span, skip logging for large bodies (>1MB) to avoid performance issues
    let html_size = html.len();
    let html_truncated = html_size > MAX_INSTRUMENTED_BODY_SIZE;
    if html_truncated {
        tracing::debug!(
            html_size_bytes = html_size,
            html_size_skipped = true,
            "HTML body exceeds 1MB, skipping detailed instrumentation"
        );
    } else {
        tracing::debug!("📄 Downloaded {} bytes from {}", html.len(), url);
    }

    // Add span field for html size (truncated)
    let span = tracing::Span::current();
    span.record("html_size_bytes", html_size.min(MAX_INSTRUMENTED_BODY_SIZE));
    span.record("html_size_skipped", html_truncated);

    // Detect WAF/CAPTCHA challenges disguised as HTTP 200 (REQ-WAF-05).
    // Context-aware inspection: status + content-type + headers drive the
    // tiered verdict, and a block carries the full Spanish evidence chain
    // (REQ-WAF-08) instead of a bare first-hit provider. `config.ignore_waf`
    // short-circuits to a clean verdict (REQ-WAF-07).
    let ctx = InspectionContext::from_lowercase_headers(
        response.status,
        &response.headers,
        config.ignore_waf,
    );
    let verdict = WafInspector::inspect(&html, &ctx);
    if verdict.is_blocked {
        let chain = verdict.evidence_chain();
        log_scrape_error(
            &chain,
            url.as_str(),
            "fetch",
            Some(&correlation),
            "WAF challenge detected",
        );
        return Err(ScraperError::waf_blocked(url.to_string(), chain));
    }

    // H1 FIX: Extract title from original DOM BEFORE any transformation.
    // This preserves the <title> tag even when --selector filters it out.
    // 'title' is a compile-time-constant selector; `Selector::parse` cannot fail.
    #[allow(clippy::expect_used)]
    let original_title = {
        let doc = scraper::Html::parse_document(&html);
        doc.select(
            &scraper::Selector::parse("title")
                // LCOV_EXCL_LINE defensive: compile-time-selector — 'title' is a constant valid selector
                .expect("invariant: 'title' is a valid CSS selector — this cannot fail"),
        )
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default()
    };

    // M7 FIX: Log selector feedback when --selector is active
    if config.selector != "body" {
        info!(
            target: "scraper",
            selector = %config.selector,
            "Aplicando selector CSS manual"
        );
    }

    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = crate::infrastructure::converter::html_cleaner::clean_html(&html);
    debug!(
        "🧹 Cleaned HTML: {} → {} bytes ({}% reduction)",
        html.len(),
        cleaned_html.len(),
        ((html.len() - cleaned_html.len()) as f64 / html.len() as f64 * 100.0).round()
    );

    // Apply CSS selector extraction if a non-default selector is configured.
    let extract_result = extract_with_selector(&cleaned_html, &config.selector, inspector);

    // Adaptive selector repair: delegate to the canonical shared helper (#442),
    // threading this use case's inspector through for diagnostics.
    #[cfg(feature = "adaptive-selectors")]
    let extract_result = adaptive_selector_repair(
        extract_result,
        engine,
        &config.selector,
        url.host_str(),
        inspector,
    )
    .await;

    let extraction_html = extract_result.as_html().to_owned();

    // Try Readability first, fallback to plain text extraction
    match crate::infrastructure::scraper::readability::parse(&extraction_html, Some(url.as_str())) {
        Ok(article) => {
            let assets = download_assets_if_enabled(
                &html,
                url,
                config,
                downloader.map(|d| d as &dyn crate::domain::ports::AssetDownloaderPort),
            )
            .await?;

            // SPA detection: check if extracted content is minimal
            if let Some(spa_info) = detect_spa_content(url.as_str(), &article.text_content, &html) {
                if spa_info.has_spa_markers {
                    warn!(
                        "{} returned minimal content ({} chars) with SPA markers detected. This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/webfang/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                } else {
                    warn!(
                        "{} returned minimal content ({} chars). This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/webfang/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                }
            }

            let author = crate::infrastructure::scraper::author_extractor::extract_author(
                &html,
                article.byline.as_deref(),
            );

            results.push(ScrapedContent {
                // H1 FIX: Use title from original DOM, falling back to Readability's title
                title: crate::application::resolve_title(
                    if original_title.is_empty() {
                        &article.title
                    } else {
                        &original_title
                    },
                    url,
                ),
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt,
                author,
                date: article.published_time,
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                // This is what downstream Markdown converters receive.
                html: Some(article.content),
                assets,
                correlation_id: Some(correlation.clone()),
            });
        },
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            // H2 FIX: Apply clean_html to fallback content to prevent JS/CSS leakage
            let raw_fallback =
                crate::infrastructure::scraper::fallback::extract_text(&extraction_html);
            let fallback_content =
                crate::infrastructure::converter::html_cleaner::clean_html(&raw_fallback);
            let assets = download_assets_if_enabled(
                &html,
                url,
                config,
                downloader.map(|d| d as &dyn crate::domain::ports::AssetDownloaderPort),
            )
            .await?;

            // SPA detection: check if fallback content is minimal
            if let Some(spa_info) = detect_spa_content(url.as_str(), &fallback_content, &html) {
                if spa_info.has_spa_markers {
                    warn!(
                        "{} returned minimal content ({} chars) with SPA markers detected. This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/webfang/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                } else {
                    warn!(
                        "{} returned minimal content ({} chars). This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/webfang/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                }
            }

            results.push(ScrapedContent {
                // H1 FIX: Use title from original DOM, falling back to host-based fallback
                title: {
                    let fallback_title = url.host_str().unwrap_or("unknown_host").to_string();
                    crate::application::resolve_title(
                        if original_title.is_empty() {
                            &fallback_title
                        } else {
                            &original_title
                        },
                        url,
                    )
                },
                content: fallback_content,
                url: ValidUrl::new(url.clone()),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html),
                assets,
                correlation_id: Some(correlation),
            });
        },
    }

    info!(
        "✅ Extracted: {} ({} chars, {} assets)",
        results
            .first()
            .map(|r| r.title.as_str())
            .unwrap_or("unknown"),
        results.first().map(|r| r.content.len()).unwrap_or(0),
        results.first().map(|r| r.assets.len()).unwrap_or(0)
    );

    Ok(ScrapeOutcome {
        results,
        extract_result,
    })
}

/// Scrape multiple URLs with concurrency control
///
/// Uses `buffer_unordered` to limit concurrent requests, preventing:
/// - File descriptor exhaustion
/// - HDD thrashing (for systems with mechanical drives)
/// - Anti-bot detection (DDoS-like patterns)
///
/// Following **config-externalize**: Concurrency is configurable via ScraperConfig.
/// Following **async-concurrency-limit**: Uses buffer_unordered for concurrency control.
///
/// # Arguments
/// * `client` - HTTP client
/// * `urls` - URLs to scrape
/// * `config` - Scraper configuration
///
/// # Returns
/// * `Vec<ScrapedContent>` - All successfully scraped content
///
/// # Note
/// Failed URLs are logged but don't stop the entire batch.
#[instrument(
    name = "scrape_multiple_with_limit",
    skip(client, urls, config, downloader),
    fields(
        urls = urls.len(),
        concurrency = config.scraper_concurrency
    )
)]
pub async fn scrape_multiple_with_limit(
    client: &dyn HttpClientPort,
    urls: &[url::Url],
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
) -> Result<Vec<ScrapedContent>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }

    // One run-root identity for the whole batch (#501): every
    // `scrape_with_config` call derives its own child from it, so the trace
    // is shared across the batch while each page span stays unique.
    let root_correlation = CorrelationId::new();
    info!(
        correlation_id = %root_correlation,
        trace_id = %root_correlation.trace_id(),
        "scrape_multiple identity"
    );

    info!(
        "🌐 Scraping {} URLs with concurrency limit {}",
        urls.len(),
        config.scraper_concurrency
    );

    let results: Vec<Result<ScrapeOutcome>> =
        stream::iter(urls.to_vec())
            .map(|url| {
                let config = config.clone();
                let root = root_correlation.clone();
                async move {
                    scrape_with_config(client, &url, &config, downloader, None, None, &root).await
                }
            })
            .buffer_unordered(config.scraper_concurrency)
            .collect()
            .await;

    let mut all_content = Vec::new();
    for result in results {
        match result {
            Ok(outcome) => all_content.extend(outcome.results),
            Err(e) => warn!("⚠️  Failed to scrape URL: {}", e),
        }
    }

    info!(
        "✅ Scraped {} pages from {} URLs",
        all_content.len(),
        urls.len()
    );
    Ok(all_content)
}
