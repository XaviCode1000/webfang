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

use crate::domain::{DownloadedAsset, ScrapedContent, ValidUrl};
use crate::error::{Result, ScraperError};
use crate::infrastructure::http::waf_engine::WafInspector;
use crate::ScraperConfig;
use futures::stream::{self, StreamExt};
use tracing::{debug, info, instrument, warn};
use wreq::Client;

/// Maximum HTML body size to log/instrument (1MB)
/// Bodies larger than this are skipped to avoid performance issues
const MAX_INSTRUMENTED_BODY_SIZE: usize = 1_048_576;

/// Minimum character threshold for considering content "substantial".
/// Pages below this threshold after extraction likely require JS rendering.
const MIN_CONTENT_CHARS: usize = 50;

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
    client: &Client,
    url: &url::Url,
) -> Result<Vec<ScrapedContent>> {
    scrape_with_config(client, url, &ScraperConfig::default()).await
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
    skip(client, config),
    fields(
        url = %url,
        has_downloads = config.has_downloads()
    )
)]
pub async fn scrape_with_config(
    client: &Client,
    url: &url::Url,
    config: &ScraperConfig,
) -> Result<Vec<ScrapedContent>> {
    let mut results = Vec::new();

    info!("🌐 Fetching: {}", url);

    let response = match client.get(url.as_str()).send().await {
        Ok(resp) => resp,
        Err(e) => return Err(ScraperError::Network(e.to_string())),
    };

    let status = response.status();
    if !status.is_success() {
        return Err(ScraperError::http(status.as_u16(), url.as_str()));
    }

    let html = response
        .text()
        .await
        .map_err(|e| ScraperError::Network(e.to_string()))?;

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

    // Try Readability first, fallback to plain text extraction
    match crate::infrastructure::scraper::readability::parse(&cleaned_html, Some(url.as_str())) {
        Ok(article) => {
            let assets = download_assets_if_enabled(&html, url, config).await?;

            // SPA detection: check if extracted content is minimal
            if let Some(spa_info) = detect_spa_content(url.as_str(), &article.text_content, &html) {
                if spa_info.has_spa_markers {
                    warn!(
                        "{} returned minimal content ({} chars) with SPA markers detected. This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/rust-scraper/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                } else {
                    warn!(
                        "{} returned minimal content ({} chars). This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/rust-scraper/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                }
            }

            results.push(ScrapedContent {
                title: crate::application::resolve_title(&article.title, url),
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                // Store CLEAN HTML from Readability (not raw HTML with nav/ads/footer)
                // This is what downstream Markdown converters receive.
                html: Some(article.content),
                assets,
            });
        },
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            let fallback_content = crate::infrastructure::scraper::fallback::extract_text(&html);
            let assets = download_assets_if_enabled(&html, url, config).await?;

            // SPA detection: check if fallback content is minimal
            if let Some(spa_info) = detect_spa_content(url.as_str(), &fallback_content, &html) {
                if spa_info.has_spa_markers {
                    warn!(
                        "{} returned minimal content ({} chars) with SPA markers detected. This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/rust-scraper/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                } else {
                    warn!(
                        "{} returned minimal content ({} chars). This site may require JavaScript rendering. This feature is not yet implemented. Track: https://github.com/XaviCode1000/rust-scraper/issues/16",
                        spa_info.url, spa_info.char_count
                    );
                }
            }

            results.push(ScrapedContent {
                title: url
                    .host_str()
                    .ok_or_else(|| ScraperError::invalid_url(format!("URL missing host: {url}")))?
                    .to_string(),
                content: fallback_content,
                url: ValidUrl::new(url.clone()),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html),
                assets,
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
    client: &Client,
    urls: &[url::Url],
    config: &ScraperConfig,
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
            let client = client.clone();
            let config = config.clone();
            let url = url.clone();
            async move { scrape_with_config(&client, &url, &config).await }
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
/// Delegates to infrastructure layer for actual downloading.
async fn download_assets_if_enabled(
    _html: &str,
    _base_url: &url::Url,
    _config: &ScraperConfig,
) -> Result<Vec<DownloadedAsset>> {
    if !_config.has_downloads() {
        return Ok(Vec::new());
    }

    #[cfg(any(feature = "images", feature = "documents"))]
    {
        crate::infrastructure::scraper::asset_download::download_all(_html, _base_url, _config)
            .await
    }

    #[cfg(not(any(feature = "images", feature = "documents")))]
    {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::http_client::create_http_client;

    #[cfg_attr(miri, ignore = "boring-sys2 FFI (wreq Client) not supported by Miri")]
    #[tokio::test]
    async fn test_scrape_with_config_invalid_url() {
        let client = create_http_client().unwrap();
        let url = url::Url::parse("https://invalid-host-that-does-not-exist-12345.com").unwrap();
        let config = ScraperConfig::default();

        let result = scrape_with_config(&client, &url, &config).await;
        // Should fail gracefully, not panic
        assert!(result.is_err());
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
}
