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
use crate::domain::http_port::HttpResponse;
use crate::domain::{CorrelationId, DomInspectorPort, ExtractResult, ScrapedContent, ValidUrl};
use crate::error::{Result, ScraperError};
use crate::infrastructure::crawler::robots_utils::RobotsFetcher;
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

/// A failed URL in a batch scrape operation.
///
/// Carries the URL and a serializable error representation so the MCP layer
/// can expose per-URL failures to the calling AI agent (issue #591).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScrapeFailed {
    /// The URL that failed to scrape.
    pub url: String,
    /// User-facing error message (Spanish, matching `ScraperError` Display).
    pub error: String,
    /// Operational classification of the error for retry/abort decisions.
    pub category: ScrapeErrorCategory,
}

/// Serializable classification of a [`ScraperError`] for MCP consumers.
///
/// Mirrors [`crate::error::ErrorClass`] but owned (no lifetime) and
/// serializable, so the MCP layer can embed it in JSON responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrapeErrorCategory {
    /// Transient — safe to retry immediately (5xx, connection reset).
    TransientRetriable,
    /// Transient — wait before retry (429, rate limit, timeout).
    TransientBackoff,
    /// Permanent — retry will not help (404, invalid URL, WAF).
    PermanentFatal,
    /// Internal — indicates a bug, not a runtime condition.
    InternalFatal,
    /// Domain recoverable — single item failed but pipeline is healthy.
    DomainRecoverable,
}

impl From<crate::error::ErrorClass> for ScrapeErrorCategory {
    fn from(class: crate::error::ErrorClass) -> Self {
        match class {
            crate::error::ErrorClass::TransientRetriable => Self::TransientRetriable,
            crate::error::ErrorClass::TransientBackoff => Self::TransientBackoff,
            crate::error::ErrorClass::PermanentFatal => Self::PermanentFatal,
            crate::error::ErrorClass::InternalFatal => Self::InternalFatal,
            crate::error::ErrorClass::DomainRecoverable => Self::DomainRecoverable,
        }
    }
}

/// Outcome of [`scrape_multiple_with_limit`]: successes + per-URL failures.
///
/// Replaces the bare `Vec<ScrapedContent>` return so callers can report
/// exactly which URLs failed and why (issue #591 — `scrape_batch` observability).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScrapeBatchOutcome {
    /// Successfully scraped content.
    pub results: Vec<ScrapedContent>,
    /// URLs that failed, with error details preserved.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub failed: Vec<ScrapeFailed>,
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
/// * `robots` - Optional robots.txt fetcher; when present (and `ignore_robots`
///   is false), disallowed URLs are rejected BEFORE any page fetch (#697)
/// * `ignore_robots` - Opt-out of robots.txt enforcement for this call (#697)
/// * `root_correlation` - Run-root correlation identity of the enclosing operation
///
/// # Returns
/// * `ScrapeOutcome` - Scraped content results + CSS selector extraction result
///
/// # Errors
/// Returns `ScraperError::Http` for HTTP errors, `ScraperError::Network` for
/// connection errors, and `ScraperError::WafBlocked` (provider `"robots.txt"`)
/// when the URL is disallowed by robots.txt (#697 audit decision — the variant
/// is reused because a dedicated robots variant does not exist).
// The parameter list is this use case's full dependency set: the same
// justification `scrape_urls` carries (#705) — bundling them into a struct
// would only move the identical wiring one level up.
#[allow(clippy::too_many_arguments)]
pub async fn scrape_with_config(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    inspector: Option<&dyn DomInspectorPort>,
    engine: Option<&AdaptiveSelectorEngine>,
    robots: Option<&RobotsFetcher>,
    ignore_robots: bool,
    root_correlation: &CorrelationId,
) -> Result<ScrapeOutcome> {
    // Robots.txt gate runs in the OUTER wrapper, before the span and before
    // any fetch, so a disallowed URL never touches the network (#697) and the
    // inner instrumented signature stays untouched.
    enforce_robots_policy(url, robots, ignore_robots).await?;

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

/// Enforce the robots.txt policy for one URL before any page fetch (#697).
///
/// Single source of truth for the robots decision — shared by the crawl path
/// ([`scrape_with_config`]) and the MCP direct-fetch tools via
/// `McpState::robots_denied_for` (#749): `ignore_robots` opts out, a `None`
/// fetcher means "no enforcement available" (fail-open), and a denial emits
/// `tracing::warn!("robots_txt_denied")` and returns
/// [`ScraperError::waf_blocked`] with provider `"robots.txt"` — the WafBlocked
/// variant is deliberately reused for robots.txt denials per the #705 audit
/// decision (`ScraperError` has no dedicated robots variant).
///
/// [`RobotsFetcher::is_allowed`] is FAIL-OPEN: if the robots.txt fetch itself
/// fails (network error, non-2xx, timeout), the URL is treated as allowed —
/// matching the production crawl behavior.
pub async fn enforce_robots_policy(
    url: &url::Url,
    robots: Option<&RobotsFetcher>,
    ignore_robots: bool,
) -> Result<()> {
    if ignore_robots {
        return Ok(());
    }
    let Some(fetcher) = robots else {
        return Ok(());
    };

    let domain = url.host_str().unwrap_or("unknown");
    if !fetcher.is_allowed(url.as_str(), domain).await {
        tracing::warn!(url = %url, domain = %domain, "robots_txt_denied");
        return Err(ScraperError::waf_blocked(url.to_string(), "robots.txt"));
    }
    Ok(())
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

    let response = fetch_html(client, url, &correlation).await?;

    let html: &str = &response.body;
    log_html_size(html, url);

    detect_waf(html, &response, config, url, &correlation)?;

    // H1 FIX: Extract title from original DOM BEFORE any transformation.
    // This preserves the <title> tag even when --selector filters it out.
    let original_title = extract_original_title(html);

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
    let cleaned_html = clean_html_for_scrape(html, config);

    // Apply CSS selector extraction if a non-default selector is configured.
    let extract_result = extract_with_selector(&cleaned_html, &config.selector, inspector);

    // Adaptive selector repair: delegate to the canonical shared helper (#442),
    // threading this use case's inspector through for diagnostics.
    // Returns (ExtractResult, Option<CascadeTrace>) for structural scoring (#792).
    #[cfg(feature = "adaptive-selectors")]
    let (extract_result, adaptive_trace) = adaptive_selector_repair(
        extract_result,
        engine,
        &config.selector,
        url.host_str(),
        inspector,
    )
    .await;

    // Compute structural quality hint if adaptive repair was attempted (#792).
    #[cfg(feature = "adaptive-selectors")]
    let quality_hint = adaptive_trace.and_then(|trace| {
        crate::application::structural_score::compute_quality_hint(&trace, &extract_result)
    });
    #[cfg(not(feature = "adaptive-selectors"))]
    let quality_hint = None;

    let extraction_html = extract_result.as_html().to_owned();

    // Try Readability first, fallback to plain text extraction
    let content = build_scraped_content(
        html,
        &extraction_html,
        url,
        config,
        downloader,
        &original_title,
        &correlation,
        quality_hint,
    )
    .await?;
    results.push(content);

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

/// Fetch the page HTML, logging failures and rejecting non-2xx responses.
async fn fetch_html(
    client: &dyn HttpClientPort,
    url: &url::Url,
    correlation: &CorrelationId,
) -> Result<HttpResponse> {
    let response = match client.get(url.as_str()).await {
        Ok(resp) => resp,
        Err(e) => {
            log_scrape_error(
                &e,
                url.as_str(),
                "fetch",
                Some(correlation),
                "HTTP request failed",
            );
            return Err(scraper_error_from_http(e, url.as_str()));
        },
    };

    if !(200..300).contains(&response.status) {
        return Err(ScraperError::http(response.status, url.as_str()));
    }

    Ok(response)
}

/// Log the HTML size and record it on the current span, skipping detailed
/// instrumentation for bodies larger than 1MB.
fn log_html_size(html: &str, url: &url::Url) {
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
}

/// Detect WAF/CAPTCHA challenges disguised as HTTP 200 (REQ-WAF-05).
///
/// Context-aware inspection: status + content-type + headers drive the tiered
/// verdict, and a block carries the full Spanish evidence chain (REQ-WAF-08)
/// instead of a bare first-hit provider. `config.ignore_waf` short-circuits to
/// a clean verdict (REQ-WAF-07).
fn detect_waf(
    html: &str,
    response: &HttpResponse,
    config: &ScraperConfig,
    url: &url::Url,
    correlation: &CorrelationId,
) -> Result<()> {
    let ctx = InspectionContext::from_lowercase_headers(
        response.status,
        &response.headers,
        config.ignore_waf,
    );
    let verdict = WafInspector::inspect(html, &ctx);
    if verdict.is_blocked {
        let chain = verdict.evidence_chain();
        log_scrape_error(
            &chain,
            url.as_str(),
            "fetch",
            Some(correlation),
            "WAF challenge detected",
        );
        return Err(ScraperError::waf_blocked(url.to_string(), chain));
    }
    Ok(())
}

/// Extract the `<title>` from the original DOM before any transformation.
///
/// 'title' is a compile-time-constant selector; `Selector::parse` cannot fail.
fn extract_original_title(html: &str) -> String {
    let doc = scraper::Html::parse_document(html);
    #[allow(clippy::expect_used)]
    let selector = scraper::Selector::parse("title")
        // LCOV_EXCL_LINE defensive: compile-time-selector — 'title' is a constant valid selector
        .expect("invariant: 'title' is a valid CSS selector — this cannot fail");
    doc.select(&selector)
        .next()
        .map(|el| el.text().collect::<String>())
        .unwrap_or_default()
}

/// Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
/// Readability, logging the reduction.
/// Applies DOM pre-pruning (#791) when enabled.
fn clean_html_for_scrape(html: &str, config: &ScraperConfig) -> String {
    let pruned_html = crate::application::extraction::prune_dom_if_enabled(html, config);
    let cleaned_html = crate::infrastructure::converter::html_cleaner::clean_html(&pruned_html);

    debug!(
        "🧹 Cleaned HTML: {} → {} bytes ({}:0.2% reduction)",
        html.len(),
        cleaned_html.len(),
        ((html.len() - cleaned_html.len()) as f64 / html.len() as f64 * 100.0).round()
    );
    cleaned_html
}

/// Build a [`ScrapedContent`] from the fetched HTML, trying Readability first
/// and falling back to plain-text extraction.
#[allow(clippy::too_many_arguments)]
async fn build_scraped_content(
    html: &str,
    extraction_html: &str,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    original_title: &str,
    correlation: &CorrelationId,
    quality_hint: Option<crate::domain::extraction_quality::ExtractionQualityHint>,
) -> Result<ScrapedContent> {
    match crate::infrastructure::scraper::readability::parse(extraction_html, Some(url.as_str())) {
        Ok(article) => {
            let assets = download_assets_if_enabled(
                html,
                url,
                config,
                downloader.map(|d| d as &dyn crate::domain::ports::AssetDownloaderPort),
            )
            .await?;

            // Shared minimum-content guard (#706): fail honestly on JS-shell
            // or near-empty extraction instead of returning Ok near-empty.
            crate::application::spa_detection::validate_min_content(
                url.as_str(),
                &article.text_content,
                html,
                correlation,
            )?;

            let author = crate::infrastructure::scraper::author_extractor::extract_author(
                html,
                article.byline.as_deref(),
            );

            Ok(ScrapedContent {
                // H1 FIX: Use title from original DOM, falling back to Readability's title
                title: crate::application::resolve_title(
                    if original_title.is_empty() {
                        &article.title
                    } else {
                        original_title
                    },
                    url,
                ),
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt.as_deref().map(|e| {
                    crate::domain::excerpt_repair::repair_empty_byline(e, author.as_deref())
                }),
                author,
                date: article.published_time,
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                // This is what downstream Markdown converters receive.
                html: Some(article.content),
                assets,
                correlation_id: Some(correlation.clone()),
                quality_hint: quality_hint.clone(),
            })
        },
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            // H2 FIX: Apply clean_html to fallback content to prevent JS/CSS leakage
            let raw_fallback =
                crate::infrastructure::scraper::fallback::extract_text(extraction_html);
            let fallback_content =
                crate::infrastructure::converter::html_cleaner::clean_html(&raw_fallback);
            let assets = download_assets_if_enabled(
                html,
                url,
                config,
                downloader.map(|d| d as &dyn crate::domain::ports::AssetDownloaderPort),
            )
            .await?;

            // Shared minimum-content guard (#706): the fallback branch has NO
            // content-size check today (unlike extract_content's
            // MIN_FALLBACK_CONTENT), so the guard is this branch's sole
            // authority — fail honestly on sub-threshold content.
            crate::application::spa_detection::validate_min_content(
                url.as_str(),
                &fallback_content,
                html,
                correlation,
            )?;

            let fallback_title = url.host_str().unwrap_or("unknown_host").to_string();
            Ok(ScrapedContent {
                // H1 FIX: Use title from original DOM, falling back to host-based fallback
                title: crate::application::resolve_title(
                    if original_title.is_empty() {
                        &fallback_title
                    } else {
                        original_title
                    },
                    url,
                ),
                content: fallback_content,
                url: ValidUrl::new(url.clone()),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html.to_string()),
                assets,
                correlation_id: Some(correlation.clone()),
                quality_hint: quality_hint.clone(),
            })
        },
    }
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
/// * `downloader` - Optional asset downloader
/// * `robots` - Optional robots.txt fetcher; when present (and `ignore_robots`
///   is false), disallowed URLs fail their per-URL check (#697)
/// * `ignore_robots` - Opt-out of robots.txt enforcement for this batch (#697)
///
/// # Returns
/// * `Vec<ScrapedContent>` - All successfully scraped content
///
/// # Note
/// Failed URLs are logged but don't stop the entire batch.
#[instrument(
    name = "scrape_multiple_with_limit",
    skip(client, urls, config, downloader, robots),
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
    robots: Option<&RobotsFetcher>,
    ignore_robots: bool,
) -> Result<ScrapeBatchOutcome> {
    if urls.is_empty() {
        return Ok(ScrapeBatchOutcome {
            results: Vec::new(),
            failed: Vec::new(),
        });
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

    // Pair each URL with its result so failures can report which URL failed
    // (ScrapeOutcome doesn't carry the source URL).
    let results: Vec<(url::Url, Result<ScrapeOutcome>)> = stream::iter(urls.to_vec())
        .map(|url| {
            let config = config.clone();
            let root = root_correlation.clone();
            async move {
                let result = scrape_with_config(
                    client,
                    &url,
                    &config,
                    downloader,
                    None,
                    None,
                    robots,
                    ignore_robots,
                    &root,
                )
                .await;
                (url, result)
            }
        })
        .buffer_unordered(config.scraper_concurrency)
        .collect()
        .await;

    let mut all_content = Vec::new();
    let mut failed = Vec::new();
    for (url, result) in results {
        match result {
            Ok(outcome) => all_content.extend(outcome.results),
            Err(e) => {
                let url_str = url.to_string();
                warn!("⚠️  Failed to scrape {url_str}: {e}");
                failed.push(ScrapeFailed {
                    url: url_str,
                    error: e.to_string(),
                    category: e.classify().into(),
                });
            },
        }
    }

    info!(
        "✅ Scraped {} pages from {} URLs ({} failed)",
        all_content.len(),
        urls.len(),
        failed.len()
    );
    Ok(ScrapeBatchOutcome {
        results: all_content,
        failed,
    })
}
