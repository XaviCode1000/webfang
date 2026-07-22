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

use crate::application::http_client::{HttpClientPort, HttpError};
use crate::domain::{
    DomInspectorPort, DownloadedAsset, ExtractResult, ScrapedContent, SelectorDiagnostic,
    SelectorErrorKind, ValidUrl,
};
use crate::error::{Result, ScraperError};
use crate::infrastructure::http::waf_engine::WafInspector;
use crate::ScraperConfig;
use futures::stream::{self, StreamExt};
use tracing::{debug, info, instrument, warn};

/// Convert an [`HttpError`] into a [`ScraperError`] with the URL context.
fn scraper_error_from_http(err: HttpError, url: &str) -> ScraperError {
    use crate::domain::error::CrawlError;
    let crawl_err: CrawlError = match err {
        HttpError::ClientError(code) | HttpError::ServerError(code) => CrawlError::Http {
            status: code,
            url: url.to_string(),
        },
        HttpError::Forbidden => CrawlError::Http {
            status: 403,
            url: url.to_string(),
        },
        HttpError::RateLimited(retry_after) => CrawlError::RateLimited(retry_after),
        HttpError::Timeout => CrawlError::Timeout,
        HttpError::Connection(msg) => CrawlError::Connection(msg),
        HttpError::Request(msg) => CrawlError::Internal(msg),
        HttpError::WafChallenge(provider) => CrawlError::WafChallenge {
            provider,
            kind: crate::domain::error::WafDetectionKind::BodySignature,
            url: url.to_string(),
        },
    };
    ScraperError::from(crawl_err)
}

/// Maximum HTML body size to log/instrument (1MB)
/// Bodies larger than this are skipped to avoid performance issues
pub const MAX_INSTRUMENTED_BODY_SIZE: usize = 1_048_576;

/// Minimum character threshold for considering content "substantial".
/// Pages below this threshold after extraction likely require JS rendering.
pub const MIN_CONTENT_CHARS: usize = 50;

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

/// Build a [`SelectorDiagnostic`] using the inspector, or return `None` if no
/// inspector was provided.
///
/// This helper calls `inspector.inspect()` for the DOM structure report and
/// `inspector.suggest()` for closest-match selector suggestions. It is only
/// called on the failure path (0 matches or invalid selector).
fn build_diagnostic(
    inspector: Option<&dyn DomInspectorPort>,
    document: &scraper::Html,
    error_kind: SelectorErrorKind,
    failed_selector: &str,
) -> Option<SelectorDiagnostic> {
    inspector.map(|insp| SelectorDiagnostic {
        error_kind,
        report: insp.inspect(document),
        suggestions: insp.suggest(document, failed_selector),
    })
}

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

/// Scrape a URL using Readability algorithm for clean content extraction
///
/// This is the 2026 best practice approach — uses the same algorithm as
/// Firefox Reader View to extract only meaningful content.
///
/// # Examples
///
/// ```no_run
/// use webfang::application::{create_http_client, scrape_with_readability};
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
    let outcome = scrape_with_config(client, url, &ScraperConfig::default(), None, None).await?;
    Ok(outcome.results)
}

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
/// # Arguments
/// * `client` - HTTP client
/// * `url` - URL to scrape
/// * `config` - Scraper configuration with download options
/// * `downloader` - Optional asset downloader
/// * `inspector` - Optional DOM inspector for selector diagnostics (None for non-MCP paths)
///
/// # Returns
/// * `ScrapeOutcome` - Scraped content results + CSS selector extraction result
///
/// # Errors
/// Returns `ScraperError::Http` for HTTP errors, `ScraperError::Network` for
/// connection errors.
#[instrument(
    name = "scrape_with_config",
    skip(client, config, downloader, inspector),
    fields(
        url = %url,
        has_downloads = config.has_downloads()
    )
)]
pub async fn scrape_with_config(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    inspector: Option<&dyn DomInspectorPort>,
) -> Result<ScrapeOutcome> {
    let mut results = Vec::new();

    info!("🌐 Fetching: {}", url);

    let response = match client.get(url.as_str()).await {
        Ok(resp) => resp,
        Err(e) => return Err(scraper_error_from_http(e, url.as_str())),
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

    // Detect WAF/CAPTCHA challenges disguised as HTTP 200
    if let Some(provider) = WafInspector::detect_body(&html) {
        warn!("WAF challenge detected from {}: {}", url, provider);
        return Err(ScraperError::WafBlocked {
            url: url.to_string(),
            provider: provider.to_string(),
        });
    }

    // H1 FIX: Extract title from original DOM BEFORE any transformation.
    // This preserves the <title> tag even when --selector filters it out.
    let original_title = {
        let doc = scraper::Html::parse_document(&html);
        doc.select(
            &scraper::Selector::parse("title")
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
                author: article.byline,
                date: article.published_time,
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                // This is what downstream Markdown converters receive.
                html: Some(article.content),
                assets,
                #[cfg(feature = "otel")]
                correlation_id: crate::domain::CorrelationId::from_otel_context(),
                #[cfg(not(feature = "otel"))]
                correlation_id: None,
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
                #[cfg(feature = "otel")]
                correlation_id: crate::domain::CorrelationId::from_otel_context(),
                #[cfg(not(feature = "otel"))]
                correlation_id: None,
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
pub async fn scrape_multiple_with_limit(
    client: &dyn HttpClientPort,
    urls: &[url::Url],
    config: &ScraperConfig,
    downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
) -> Result<Vec<ScrapedContent>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }

    info!(
        "🌐 Scraping {} URLs with concurrency limit {}",
        urls.len(),
        config.scraper_concurrency
    );

    let results: Vec<Result<ScrapeOutcome>> = stream::iter(urls.to_vec())
        .map(|url| {
            let config = config.clone();
            async move { scrape_with_config(client, &url, &config, downloader, None).await }
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

/// Helper: Download assets if config has downloads enabled
///
/// Uses the `AssetDownloaderPort` trait for testability.
/// Falls back to constructing a concrete `Downloader` when no trait object is provided.
pub async fn download_assets_if_enabled(
    _html: &str,
    _base_url: &url::Url,
    _config: &crate::ScraperConfig,
    _shared_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
) -> Result<Vec<DownloadedAsset>> {
    if !_config.has_downloads() {
        return Ok(Vec::new());
    }

    #[cfg(any(feature = "images", feature = "documents"))]
    {
        // Use shared downloader when provided; create a fallback one otherwise
        let owned_downloader;
        let downloader: &dyn crate::domain::ports::AssetDownloaderPort = match _shared_downloader {
            Some(dl) => dl,
            None => {
                owned_downloader =
                    crate::adapters::downloader::Downloader::new(_config.to_download_config())?;
                &owned_downloader
            },
        };

        // Extract URLs from HTML
        let mut urls: Vec<String> = Vec::new();
        {
            let document = scraper::Html::parse_document(_html);
            if _config.download_images {
                let images = crate::extractor::extract_images(&document, _base_url);
                urls.extend(images.into_iter().map(|a| a.url));
            }
            if _config.download_documents {
                let docs = crate::extractor::extract_documents(&document, _base_url);
                urls.extend(docs.into_iter().map(|a| a.url));
            }
        }

        if urls.is_empty() {
            return Ok(Vec::new());
        }

        // Deduplicate URLs to avoid downloading the same asset multiple times
        // (e.g., same image referenced from multiple <img> tags).
        use std::collections::HashSet;
        let mut seen = HashSet::with_capacity(urls.len());
        urls.retain(|url| seen.insert(url.clone()));

        tracing::info!(
            "📦 Downloading {} assets via adapters::Downloader",
            urls.len()
        );

        let results = downloader.download_batch(&urls).await?;

        // Trait impl already returns domain::DownloadedAsset — collect directly
        Ok(results)
    }

    #[cfg(not(any(feature = "images", feature = "documents")))]
    {
        Ok(Vec::new())
    }
}
