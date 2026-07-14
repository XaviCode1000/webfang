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
use crate::domain::{DownloadedAsset, ScrapedContent, ValidUrl};
use crate::error::{Result, ScraperError};
use crate::infrastructure::http::waf_engine::WafInspector;
use crate::ScraperConfig;
use futures::stream::{self, StreamExt};
use tracing::{debug, info, instrument, warn};

/// Convert an [`HttpError`] into a [`ScraperError`] with the URL context.
fn scraper_error_from_http(err: HttpError, url: &str) -> ScraperError {
    match err {
        HttpError::ClientError(code) | HttpError::ServerError(code) => {
            ScraperError::http(code, url)
        },
        HttpError::Forbidden => ScraperError::http(403, url),
        HttpError::RateLimited(retry_after) => ScraperError::Network(Box::new(
            std::io::Error::other(format!("rate limited, retry after {retry_after}s")),
        )),
        HttpError::Timeout => {
            ScraperError::Network(Box::new(std::io::Error::other("request timeout")))
        },
        HttpError::Connection(msg) => ScraperError::Network(Box::new(std::io::Error::other(msg))),
        HttpError::Request(msg) => ScraperError::Network(Box::new(std::io::Error::other(msg))),
        HttpError::WafChallenge(provider) => ScraperError::WafBlocked {
            url: url.to_string(),
            provider,
        },
    }
}

/// Maximum HTML body size to log/instrument (1MB)
/// Bodies larger than this are skipped to avoid performance issues
const MAX_INSTRUMENTED_BODY_SIZE: usize = 1_048_576;

/// Minimum character threshold for considering content "substantial".
/// Pages below this threshold after extraction likely require JS rendering.
const MIN_CONTENT_CHARS: usize = 50;

/// Extract HTML content using a CSS selector.
///
/// When `selector` is not "body", parses the HTML and extracts all elements
/// matching the selector. Returns the outer HTML of matched elements wrapped
/// in a `<div>` for Readability processing. If no elements match, returns
/// the original HTML unchanged.
pub(crate) fn extract_with_selector(html: &str, selector: &str) -> String {
    if selector == "body" {
        return html.to_owned();
    }

    let document = scraper::Html::parse_document(html);
    let sel = match scraper::Selector::parse(selector) {
        Ok(s) => s,
        Err(e) => {
            warn!(
                "Invalid CSS selector '{}': {}, falling back to full HTML",
                selector, e
            );
            return html.to_owned();
        },
    };

    let matched: Vec<String> = document.select(&sel).map(|el| el.html()).collect();

    if matched.is_empty() {
        warn!(
            "CSS selector '{}' matched 0 elements, falling back to full HTML",
            selector
        );
        return html.to_owned();
    }

    debug!(
        "CSS selector '{}' matched {} elements",
        selector,
        matched.len()
    );

    format!(
        "<div id=\"selector-extracted\">{}</div>",
        matched.join("\n")
    )
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
/// use rust_scraper::application::{create_http_client, scrape_with_readability};
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
    scrape_with_config(client, url, &ScraperConfig::default(), None).await
}

/// Scrape a URL with asset downloading configuration
///
/// # Arguments
/// * `client` - HTTP client
/// * `url` - URL to scrape
/// * `config` - Scraper configuration with download options
///
/// # Returns
/// * `Vec<ScrapedContent>` - Scraped content with downloaded assets
///
/// # Errors
/// Returns `ScraperError::Http` for HTTP errors, `ScraperError::Network` for
/// connection errors.
#[instrument(
    name = "scrape_with_config",
    skip(client, config, downloader),
    fields(
        url = %url,
        has_downloads = config.has_downloads()
    )
)]
pub async fn scrape_with_config(
    client: &dyn HttpClientPort,
    url: &url::Url,
    config: &ScraperConfig,
    downloader: Option<&crate::adapters::downloader::Downloader>,
) -> Result<Vec<ScrapedContent>> {
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
        doc.select(&scraper::Selector::parse("title").unwrap())
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
    let extraction_html = extract_with_selector(&cleaned_html, &config.selector);

    // Try Readability first, fallback to plain text extraction
    match crate::infrastructure::scraper::readability::parse(&extraction_html, Some(url.as_str())) {
        Ok(article) => {
            let assets = download_assets_if_enabled(&html, url, config, downloader).await?;

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
            let assets = download_assets_if_enabled(&html, url, config, downloader).await?;

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

    Ok(results)
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
    downloader: Option<&crate::adapters::downloader::Downloader>,
) -> Result<Vec<ScrapedContent>> {
    if urls.is_empty() {
        return Ok(Vec::new());
    }

    info!(
        "🌐 Scraping {} URLs with concurrency limit {}",
        urls.len(),
        config.scraper_concurrency
    );

    let results: Vec<Result<Vec<ScrapedContent>>> = stream::iter(urls.to_vec())
        .map(|url| {
            let config = config.clone();
            async move { scrape_with_config(client, &url, &config, downloader).await }
        })
        .buffer_unordered(config.scraper_concurrency)
        .collect()
        .await;

    let mut all_content = Vec::new();
    for result in results {
        match result {
            Ok(contents) => all_content.extend(contents),
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
/// Uses the adapters `Downloader` (streaming, pooled client, ~8KB RAM).
/// Falls back to no-op when feature gates are disabled.
pub(crate) async fn download_assets_if_enabled(
    _html: &str,
    _base_url: &url::Url,
    _config: &crate::ScraperConfig,
    _shared_downloader: Option<&crate::adapters::downloader::Downloader>,
) -> Result<Vec<DownloadedAsset>> {
    if !_config.has_downloads() {
        return Ok(Vec::new());
    }

    #[cfg(any(feature = "images", feature = "documents"))]
    {
        // Use shared downloader when provided; create a fallback one otherwise
        let owned_downloader;
        let downloader = match _shared_downloader {
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

        let results = downloader.download_batch(&urls).await;

        // Convert adapters::DownloadedAsset → domain::DownloadedAsset
        let mut assets = Vec::new();
        for result in results {
            match result {
                Ok(asset) => {
                    let asset_type = crate::adapters::detector::detect_from_url(&asset.url);
                    let asset_type_str = match asset_type {
                        crate::adapters::detector::AssetType::Image => "image",
                        crate::adapters::detector::AssetType::Document => "document",
                        crate::adapters::detector::AssetType::Unknown => "unknown",
                    };
                    assets.push(DownloadedAsset {
                        url: asset.url,
                        local_path: asset.local_path.to_string_lossy().into_owned(),
                        asset_type: asset_type_str.to_string(),
                        size: asset.size,
                    });
                },
                Err(e) => {
                    tracing::warn!("Failed to download asset: {}", e);
                },
            }
        }

        Ok(assets)
    }

    #[cfg(not(any(feature = "images", feature = "documents")))]
    {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::http_client::port::{HttpClientPort, HttpResponse};
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_scrape_with_config_invalid_url() {
        let url = url::Url::parse("https://invalid-host-that-does-not-exist-12345.com").unwrap();
        let config = ScraperConfig::default();
        let mock = MockHttpClient::new().with_response(
            url.as_str(),
            Err(HttpError::Connection("no route to host".into())),
        );

        let result = scrape_with_config(&mock, &url, &config, None).await;
        assert!(result.is_err(), "connection error should propagate as Err");
    }

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
        let result = detect_spa_content("https://example.com", "This is a substantial content that exceeds the minimum threshold of 50 characters easily.", "<html><body>Content</body></html>");
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

    // --- Mock-based tests for HttpClientPort integration ---

    struct MockHttpClient {
        responses: HashMap<String, crate::application::http_client::HttpResult<HttpResponse>>,
    }

    impl MockHttpClient {
        fn new() -> Self {
            Self {
                responses: HashMap::new(),
            }
        }

        fn with_response(
            mut self,
            url: &str,
            result: crate::application::http_client::HttpResult<HttpResponse>,
        ) -> Self {
            self.responses.insert(url.to_string(), result);
            self
        }

        /// Shorthand for a 200 OK response with the given HTML body.
        fn with_ok_response(self, url: &str, body: &str) -> Self {
            self.with_response(
                url,
                Ok(HttpResponse {
                    status: 200,
                    body: body.to_string(),
                    headers: HashMap::new(),
                }),
            )
        }
    }

    impl HttpClientPort for MockHttpClient {
        fn get(
            &self,
            url: &str,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = crate::application::http_client::HttpResult<HttpResponse>,
                    > + Send
                    + '_,
            >,
        > {
            let result = self
                .responses
                .get(url)
                .cloned()
                .unwrap_or(Err(HttpError::ClientError(404)));
            Box::pin(async move { result })
        }
    }

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
        let mock =
            MockHttpClient::new().with_response(url.as_str(), Err(HttpError::ClientError(404)));

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
        let mock =
            MockHttpClient::new().with_response(url.as_str(), Err(HttpError::ServerError(500)));

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
        let mock =
            MockHttpClient::new().with_response(url.as_str(), Err(HttpError::RateLimited(60)));

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

    // -- Mutation-killing tests for scraper_service --

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
        let result = extract_with_selector(html, "body");
        assert_eq!(
            result, html,
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
        let result = extract_with_selector(html, "div.main");
        assert!(
            result.contains("Main content"),
            "should contain matched element content"
        );
        assert!(
            result.contains("selector-extracted"),
            "should wrap in selector-extracted div"
        );
        assert!(
            !result.contains("Sidebar"),
            "should NOT contain unmatched element content"
        );
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
    #[test]
    fn test_extract_with_selector_no_matches_falls_back() {
        let html = "<html><body><p>Hello</p></body></html>";
        let result = extract_with_selector(html, "article");
        assert_eq!(result, html, "no matches should fall back to original HTML");
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
    #[test]
    fn test_extract_with_selector_invalid_syntax_falls_back() {
        let html = "<html><body><p>Hello</p></body></html>";
        let result = extract_with_selector(html, ">>>invalid");
        assert_eq!(
            result, html,
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
        let result = extract_with_selector(html, "li");
        assert!(result.contains("Item 1"));
        assert!(result.contains("Item 2"));
        assert!(result.contains("Item 3"));
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector drop triggers servo_arc Tree-Borrows UB under Miri
    #[test]
    fn test_extract_with_selector_id_selector() {
        let html = r#"<html><body>
            <div id="target"><p>Targeted</p></div>
            <div id="other"><p>Other</p></div>
        </body></html>"#;
        let result = extract_with_selector(html, "#target");
        assert!(result.contains("Targeted"));
        assert!(!result.contains("Other"));
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
}
