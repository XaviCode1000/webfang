//! Modern Scraper Module
//!
//! Uses reqwest for HTTP and legible (Readability algorithm) for clean content extraction.
//! This is the 2026 best practice approach for obtaining clean data for RAG/datasets.

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tracing::{debug, info, warn};

/// HTTP Client configuration
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const TIMEOUT_SECS: u64 = 30;

/// Represents a scraped content item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapedContent {
    /// Title of the page/article
    pub title: String,
    /// Main content extracted (clean, without ads/nav)
    pub content: String,
    /// Original URL
    pub url: String,
    /// Excerpt/summary if available
    pub excerpt: Option<String>,
    /// Author if available
    pub author: Option<String>,
    /// Publication date if available
    pub date: Option<String>,
    /// The HTML source (optional, for debugging)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

/// Create configured HTTP client with best practices
pub fn create_http_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .gzip(true) // Most modern sites use gzip
        .brotli(true)
        .build()
        .context("Failed to create HTTP client")
}

/// Scrape a URL using Readability algorithm for clean content extraction
///
/// This is the modern 2026 approach - uses the same algorithm as Firefox Reader View
/// to extract only the meaningful content (article body), filtering out:
/// - Navigation menus
/// - Advertisements
/// - Sidebars
/// - Footer content
/// - Scripts and styles
pub async fn scrape_with_readability(
    client: &Client,
    url: &url::Url,
    _selector: &str, // Reserved for future advanced selectors
    _max_pages: usize,
    _delay_ms: u64,
) -> Result<Vec<ScrapedContent>> {
    let mut results = Vec::new();

    // For now, scrape single URL - can extend to crawl later
    info!("🌐 Fetching: {}", url);

    // Fetch HTML
    let response = client
        .get(url.as_str())
        .send()
        .await
        .with_context(|| format!("Failed to fetch URL: {}", url))?;

    // Check status
    let status = response.status();
    if !status.is_success() {
        anyhow::bail!("HTTP error: {} - {}", status, url);
    }

    // Get HTML content
    let html = response
        .text()
        .await
        .context("Failed to read response body")?;

    debug!("📄 Downloaded {} bytes from {}", html.len(), url);

    // Extract clean content using Readability algorithm
    // legible::parse requires url and options as arguments
    match legible::parse(&html, Some(url.as_str()), None) {
        Ok(article) => {
            let content = ScrapedContent {
                // legible uses fields, not methods
                title: article.title,
                content: article.text_content,
                url: url.to_string(),
                excerpt: article.excerpt,
                author: article.byline,
                date: article.published_time,
                html: Some(html), // Keep for debugging if needed
            };

            info!(
                "✅ Extracted: {} ({} chars)",
                content.title,
                content.content.len()
            );
            results.push(content);
        }
        Err(e) => {
            warn!("⚠️  Readability failed for {}: {}", url, e);
            // Try fallback: just extract text directly
            let fallback_content = extract_fallback_text(&html);
            results.push(ScrapedContent {
                title: url.host_str().unwrap_or("Unknown").to_string(),
                content: fallback_content,
                url: url.to_string(),
                excerpt: None,
                author: None,
                date: None,
                html: Some(html),
            });
        }
    }

    // Note: For multi-page crawling, implement delay and loop here
    // For now, single page as per max_pages = 1 for simplicity

    Ok(results)
}

/// Fallback: Extract text without readability (basic HTML stripping)
fn extract_fallback_text(html: &str) -> String {
    htmd::convert(html).unwrap_or_else(|_| {
        // If htmd fails, do a very basic strip
        html.lines()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Save scraped results to output directory
pub fn save_results(
    results: &[ScrapedContent],
    output_dir: &PathBuf,
    format: &super::OutputFormat,
) -> Result<()> {
    use std::fs;

    // Create output directory
    fs::create_dir_all(output_dir)?;

    match format {
        super::OutputFormat::Markdown => {
            for (i, item) in results.iter().enumerate() {
                let filename = format!("doc_{:03}.md", i);
                let path = output_dir.join(&filename);

                let md_content = format!(
                    "# {}\n\n{}\n\n---\n\n*Source: [{}]({})*",
                    item.title, item.content, item.url, item.url
                );

                fs::write(&path, md_content)?;
                info!("💾 Saved: {}", path.display());
            }
        }
        super::OutputFormat::Text => {
            for (i, item) in results.iter().enumerate() {
                let filename = format!("doc_{:03}.txt", i);
                let path = output_dir.join(&filename);
                fs::write(&path, &item.content)?;
                info!("💾 Saved: {}", path.display());
            }
        }
        super::OutputFormat::Json => {
            let json_path = output_dir.join("results.json");
            let json = serde_json::to_string_pretty(results)?;
            fs::write(&json_path, json)?;
            info!("💾 Saved: {}", json_path.display());
        }
    }

    Ok(())
}

// ============================================================================
// Legacy functions removed in v0.2.0
// ============================================================================

/// Legacy function - removed
#[allow(dead_code)]
fn _deprecated_crawl_target() {
    // This function no longer exists - use scrape_with_readability instead
    // Keeping empty to avoid breaking builds that might reference it
}
