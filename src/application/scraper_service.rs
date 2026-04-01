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

use super::http_client::detect_waf_challenge;
use crate::domain::{DownloadedAsset, ScrapedContent, ValidUrl};
use crate::error::{Result, ScraperError};
use crate::ScraperConfig;
use futures::stream::{self, StreamExt};
use tracing::{debug, info, warn};
use wreq::Client;

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
    debug!("📄 Downloaded {} bytes from {}", html.len(), url);

    // Detect WAF/CAPTCHA challenges disguised as HTTP 200
    if let Some(provider) = detect_waf_challenge(&html) {
        warn!("WAF challenge detected from {}: {}", url, provider);
        return Err(ScraperError::WafBlocked {
            url: url.to_string(),
            provider: provider.to_string(),
        });
    }

    // Try Readability first, fallback to plain text extraction
    match crate::infrastructure::scraper::readability::parse(&html, Some(url.as_str())) {
        Ok(article) => {
            let assets = download_assets_if_enabled(&html, url, config).await?;

            results.push(ScrapedContent {
                title: article.title,
                content: article.text_content,
                url: ValidUrl::new(url.clone()),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                html: Some(html),
                assets,
            });
        }
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            let fallback_content = crate::infrastructure::scraper::fallback::extract_text(&html);
            let assets = download_assets_if_enabled(&html, url, config).await?;

            results.push(ScrapedContent {
                title: url
                    .host_str()
                    .ok_or_else(|| ScraperError::invalid_url(format!("URL missing host: {}", url)))?
                    .to_string(),
                content: fallback_content,
                url: ValidUrl::new(url.clone()),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html),
                assets,
            });
        }
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

    let tasks = urls.iter().map(|url| {
        let client = client.clone();
        let config = config.clone();
        async move { scrape_with_config(&client, url, &config).await }
    });

    let results: Vec<Result<Vec<ScrapedContent>>> = stream::iter(tasks)
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
}
