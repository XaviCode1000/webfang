//! Discovery module — URL discovery and sitemap parsing
//!
//! Functions for discovering URLs from websites via sitemaps or DOM scraping.
//! Part of the TUI workflow: discover → select → scrape.

use anyhow::Result;
use tracing::{debug, info, instrument, span, warn, Level};
use url::Url;

use crate::application::url_filter::is_allowed;
use crate::domain::{CrawlError, CrawlerConfig, DiscoveredUrl, ScrapedContent, ValidUrl};
use crate::error::{Result as ScraperResult, ScraperError};
use crate::infrastructure::crawler::binary_utils::derive_filename_from_response;
use crate::infrastructure::crawler::{
    extract_links, is_internal_link, normalize_url, SitemapConfig, SitemapParser,
};
use crate::infrastructure::scraper::{fallback, readability};
use crate::ScraperConfig;

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    CRAWLER_BANDWIDTH, CRAWLER_PAGES, CRAWLER_URLS,
};

// ============================================================================
// TUI Support — Discover/Scrape Use Cases
// ============================================================================

/// Discover URLs from a website without downloading content
///
/// This is the first step in the TUI workflow:
/// 1. Discover all URLs from sitemap or DOM scraping
/// 2. Return Vec<Url> for interactive selection
/// 3. User selects which URLs to scrape
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
///
/// # Arguments
///
/// * `base_url` - Base URL to discover from
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(Vec<Url>)` - Discovered URLs (owned)
/// * `Err(anyhow::Error)` - Error during discovery
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::{application::discover_urls_for_tui, domain::CrawlerConfig};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::new(seed);
///
/// let urls = discover_urls_for_tui("https://example.com", &config).await?;
/// println!("Found {} URLs", urls.len());
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "discover_urls_for_tui",
    skip(config),
    fields(
        base_url,
        use_sitemap = config.use_sitemap
    )
)]
pub async fn discover_urls_for_tui(
    base_url: &str,
    config: &CrawlerConfig,
) -> anyhow::Result<Vec<Url>> {
    let span = span!(Level::INFO, "discover_urls", base_url = base_url);
    let _guard = span.enter();

    info!("Discovering URLs from {}", base_url);

    // If sitemap enabled, use sitemap (preferred)
    if config.use_sitemap {
        let discovered =
            crawl_with_sitemap(base_url, config.sitemap_url.as_deref(), config).await?;
        let urls: Vec<Url> = discovered.into_iter().map(|d| d.url).collect();

        #[cfg(feature = "otel-metrics")]
        CRAWLER_URLS.add(urls.len() as u64, &[]);

        Ok(urls)
    } else {
        // DOM scraping - extract links from single page
        let client = super::super::create_http_client()?;

        info!("Fetching {} for link extraction", base_url);
        let response = client
            .get(base_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP error: {e}"))?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or("unknown"))
            .unwrap_or("unknown");
        let content_length = response
            .headers()
            .get("content-length")
            .map(|v| v.to_str().unwrap_or("0"))
            .unwrap_or("0");

        debug!(
            "Response: status={}, content-type={}, content-length={}",
            status, content_type, content_length
        );

        let html = response
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("Network error: {e}"))?;

        debug!("Received HTML: {} bytes", html.len());

        let base = Url::parse(base_url).map_err(|e| anyhow::anyhow!("Invalid URL: {e}"))?;

        // Extract links
        let links =
            extract_links(&html, base_url).map_err(|e| anyhow::anyhow!("Parse error: {e}"))?;

        // Filter and normalize URLs
        let mut urls = Vec::new();
        for link in links {
            let normalized = normalize_url(&link);
            if let Ok(parsed_url) = Url::parse(&normalized) {
                // Check if internal link
                if let Some(seed_domain) = base.host_str() {
                    if is_internal_link(&normalized, seed_domain) {
                        // Check if allowed by filters
                        if is_allowed(&normalized, config) {
                            urls.push(parsed_url);
                        }
                    }
                }
            }
        }

        info!("Discovered {} URLs from {}", urls.len(), base_url);

        #[cfg(feature = "otel-metrics")]
        CRAWLER_URLS.add(urls.len() as u64, &[]);

        Ok(urls)
    }
}

/// Scrape/download specific URLs
///
/// This is the second step in the TUI workflow:
/// 1. User selects URLs via TUI
/// 2. This function downloads and extracts content
///
/// Following **own-borrow-over-clone**: Accepts `&[Url]` not `&Vec<Url>`.
/// Following **async-no-lock-across-await**: Uses stream with buffer_unordered.
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
///
/// # Arguments
///
/// * `urls` - Slice of URLs to scrape (borrowed)
/// * `config` - Scraper configuration
///
/// # Returns
///
/// * `Ok(Vec<ScrapedContent>)` - Scraped content from each URL
/// * `Err(ScraperError)` - Error during scraping
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::{application::scrape_urls_for_tui, ScraperConfig};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let urls = vec![
///     Url::parse("https://example.com/1")?,
///     Url::parse("https://example.com/2")?,
/// ];
/// let config = ScraperConfig::default();
/// let results = scrape_urls_for_tui(&urls, &config, None).await?;
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "scrape_urls_for_tui",
    skip(urls, config, downloader),
    fields(
        url_count = urls.len(),
        concurrency = config.scraper_concurrency,
        has_downloads = config.has_downloads()
    )
)]
pub async fn scrape_urls_for_tui(
    urls: &[Url],
    config: &ScraperConfig,
    downloader: Option<&crate::adapters::downloader::Downloader>,
) -> ScraperResult<Vec<ScrapedContent>> {
    use futures::stream::{self, StreamExt};

    let span = span!(Level::INFO, "scrape_urls", count = urls.len());
    let _guard = span.enter();

    info!("Scraping {} URLs", urls.len());

    let client = super::super::create_http_client()?;

    // Stream processing with concurrency control
    // Following async-no-lock-across-await: buffer_unordered handles concurrency
    let results = stream::iter(urls)
        .map(|url| async { scrape_single_url_for_tui(&client, url, config, downloader).await })
        .buffered(config.scraper_concurrency)
        .collect::<Vec<_>>()
        .await;

    // Collect results, propagating first error if any
    results.into_iter().collect()
}

/// Scrape a single URL
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
///
/// # Arguments
///
/// * `client` - HTTP client to use for requests
/// * `url` - URL to scrape
/// * `config` - Scraper configuration
///
/// # Returns
///
/// * `Ok(ScrapedContent)` - Scraped content from the URL
/// * `Err(ScraperError)` - Error during scraping
#[instrument(
    name = "scrape_single_url",
    skip(client, config, downloader),
    fields(url = %url)
)]
pub async fn scrape_single_url_for_tui(
    client: &wreq::Client,
    url: &Url,
    config: &ScraperConfig,
    downloader: Option<&crate::adapters::downloader::Downloader>,
) -> ScraperResult<ScrapedContent> {
    let span = span!(Level::DEBUG, "scrape_single", url = %url);
    let _guard = span.enter();

    debug!("Scraping: {}", url);

    // Fetch HTML
    let response = client
        .get(url.as_str())
        .send()
        .await
        .map_err(|e| ScraperError::Network(Box::new(e)))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ScraperError::http(status.as_u16(), url.as_str()));
    }

    // Check content-type before reading body to handle binary content (PDFs, etc.)
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let is_binary = content_type.contains("application/pdf")
        || content_type.contains("application/octet-stream")
        || content_type.contains("application/zip")
        || content_type.contains("application/x-")
        || content_type.contains("image/")
        || content_type.contains("audio/")
        || content_type.contains("video/");

    if is_binary {
        debug!("Binary content type detected: {} for {}", content_type, url);

        // Save binary file when download_documents is enabled
        let saved_path = if config.download_documents {
            let filename = derive_filename_from_response(response.headers(), url, &content_type);
            let output_path = config.output_dir.join(&filename);

            match response.bytes().await {
                Ok(bytes) => {
                    if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
                        warn!(
                            "Failed to create output directory {}: {}",
                            config.output_dir.display(),
                            e
                        );
                    } else if let Err(e) = std::fs::write(&output_path, &bytes) {
                        warn!(
                            "Failed to save binary file {}: {}",
                            output_path.display(),
                            e
                        );
                    } else {
                        info!(
                            "Saved binary file: {} ({} bytes)",
                            output_path.display(),
                            bytes.len()
                        );
                    }
                    Some(output_path)
                },
                Err(e) => {
                    warn!("Failed to read binary response for {}: {}", url, e);
                    None
                },
            }
        } else {
            let _ = response.bytes().await;
            None
        };

        let assets = crate::application::scraper_service::download_assets_if_enabled(
            "", url, config, downloader,
        )
        .await?;

        let content = if let Some(ref path) = saved_path {
            format!("[Binary file saved: {}] {}", path.display(), url.as_str())
        } else {
            format!("[Binary content: {content_type}] {}", url.as_str())
        };

        return Ok(ScrapedContent {
            title: url
                .host_str()
                .ok_or_else(|| ScraperError::invalid_url(format!("URL missing host: {url}")))?
                .to_string(),
            content,
            url: ValidUrl::new(url.clone()),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets,
            correlation_id: None,
        });
    }

    let html = response
        .text()
        .await
        .map_err(|e| ScraperError::Network(Box::new(e)))?;

    #[cfg(feature = "otel-metrics")]
    {
        CRAWLER_BANDWIDTH.add(
            html.len() as u64,
            &[opentelemetry::KeyValue::new("url", url.to_string())],
        );
    }

    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = crate::infrastructure::converter::html_cleaner::clean_html(&html);

    // Apply CSS selector extraction if a non-default selector is configured.
    let extraction_html =
        crate::application::scraper_service::extract_with_selector(&cleaned_html, &config.selector);

    // Try Readability first, fallback to plain text extraction
    match readability::parse(&extraction_html, Some(url.as_str())) {
        Ok(article) => {
            #[cfg(feature = "otel-metrics")]
            CRAWLER_PAGES.add(1, &[opentelemetry::KeyValue::new("method", "readability")]);

            let assets = crate::application::scraper_service::download_assets_if_enabled(
                &html, url, config, downloader,
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
                correlation_id: None,
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
                warn!("{}", msg);
                return Err(ScraperError::ExtractionFailed {
                    url: url.to_string(),
                    reason: msg,
                });
            }

            let assets = crate::application::scraper_service::download_assets_if_enabled(
                &html, url, config, downloader,
            )
            .await?;

            #[cfg(feature = "otel-metrics")]
            CRAWLER_PAGES.add(1, &[opentelemetry::KeyValue::new("method", "fallback")]);

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
                html: Some(html),
                assets,
                correlation_id: None,
            })
        },
    }
}

// ============================================================================
// Sitemap Discovery
// ============================================================================

/// Crawl site using sitemap (preferred method - FASE 3)
///
/// Following **err-anyhow-for-applications**: Uses anyhow::Result.
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **api-builder-pattern**: Uses SitemapConfig builder.
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
/// * `sitemap_url` - Optional explicit sitemap URL (auto-discovers if None)
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(Vec<DiscoveredUrl>)` - URLs discovered from sitemap
/// * `Err(CrawlError)` - Error during sitemap fetch or parse
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::application::crawl_with_sitemap;
/// use rust_scraper::domain::CrawlerConfig;
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> anyhow::Result<()> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::new(seed);
///
/// let urls = crawl_with_sitemap("https://example.com", None, &config).await?;
/// println!("Found {} URLs from sitemap", urls.len());
/// # Ok(())
/// # }
/// ```
pub async fn crawl_with_sitemap(
    base_url: &str,
    sitemap_url: Option<&str>,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    let span = span!(Level::INFO, "crawl_with_sitemap", base_url = base_url);
    let _guard = span.enter();

    crawl_with_sitemap_internal(base_url, sitemap_url, config).await
}

/// Crawl with sitemap (internal version with progress tracking)
///
/// This is the internal implementation that supports optional progress tracking.
/// The public `crawl_with_sitemap` function calls this one.
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **err-anyhow-for-applications**: Uses Result with anyhow.
#[allow(unused_variables)]
async fn crawl_with_sitemap_internal(
    base_url: &str,
    sitemap_url: Option<&str>,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    info!("Crawling with sitemap for {}", base_url);

    // Use default batch size (10,000) - SitemapConfig handles pagination
    // CrawlerConfig doesn't have batch_size, we use SitemapConfig for that
    const DEFAULT_BATCH_SIZE: usize = 10_000;

    // Auto-discover sitemap URL if not provided
    let sitemap_url = match sitemap_url {
        Some(url) if !url.is_empty() => {
            tracing::info!("Sitemap URL provided: {}", url);
            url.to_string()
        },
        _ => {
            tracing::info!("Auto-discovering sitemap URL for {}", base_url);
            match discover_sitemap_url(base_url).await {
                Ok(url) => {
                    tracing::info!("Discovered sitemap URL: {}", url);
                    url
                },
                Err(CrawlError::SitemapNotFound(_)) => {
                    tracing::warn!("no sitemap found, switching to standard crawling");
                    return Ok(Vec::new());
                },
                Err(e) => return Err(e),
            }
        },
    };

    tracing::info!("Using sitemap: {}", sitemap_url);

    // Create sitemap parser with config (including pagination settings)
    // Following api-builder-pattern: builder API
    let parser = SitemapParser::with_config(
        SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(3)
            .concurrency(5)
            .batch_size(DEFAULT_BATCH_SIZE)
            .pagination_enabled(true)
            .build(),
    );

    // Parse sitemap
    let urls = parser.parse_from_url(&sitemap_url).await.map_err(|e| {
        tracing::error!("Failed to parse sitemap {}: {}", sitemap_url, e);
        CrawlError::Sitemap(e.to_string())
    })?;

    let total_urls = urls.len();
    tracing::info!("Parsed {} total URLs from sitemap", total_urls);

    // Validate sitemap relevance: check if any URLs share a path prefix
    // with the target URL. This handles cases where robots.txt points to
    // an unrelated sitemap (e.g. blog sitemap for a docs site).
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
    let target_path = base.path().to_string();
    let relevant_urls: Vec<_> = urls
        .into_iter()
        .filter(|url| url.path().starts_with(&target_path))
        .collect();

    // If no relevant URLs found, try sub-path sitemaps as fallback
    if relevant_urls.is_empty() {
        tracing::warn!(
            "sitemap {} no tiene URLs que coincidan con la ruta objetivo {}, intentando sitemaps de subruta",
            sitemap_url,
            target_path
        );
        return crawl_with_subpath_sitemaps(base_url, &base, &parser, 3, 0).await;
    }

    // Following own-borrow-over-clone: use Url directly, not String
    // Use explicit type annotation for type inference
    // Apply include/exclude patterns from config (Fix: sitemap URLs were bypassing filters)
    let discovered: Vec<DiscoveredUrl> = relevant_urls
        .into_iter()
        .filter(|url| is_allowed(url.as_str(), config))
        .map(|url| DiscoveredUrl::html(url, 0, base.clone()))
        .collect();

    #[cfg(feature = "otel-metrics")]
    CRAWLER_URLS.add(discovered.len() as u64, &[]);

    Ok(discovered)
}

/// Try sub-path sitemaps when the discovered sitemap has no relevant URLs
///
/// For nested sites like `https://example.com/docs/en/`, this tries
/// `/docs/sitemap.xml`, `/docs/en/sitemap.xml`, etc.
/// Follows nested sitemaps recursively up to `max_depth` levels.
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
/// Following **err-no-unwrap-prod**: Proper error handling throughout.
async fn crawl_with_subpath_sitemaps(
    base_url: &str,
    base: &Url,
    parser: &SitemapParser,
    max_depth: usize,
    current_depth: usize,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    if current_depth >= max_depth {
        tracing::warn!(
            "sitemap recursion depth {} reached max {}, stopping",
            current_depth,
            max_depth
        );
        return Ok(Vec::new());
    }

    let path = base.path();
    let segments: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
    let mut all_urls = Vec::new();

    // Try up to 3 path levels: /docs, /docs/en, /docs/en/quickstart
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for sitemap_name in &["sitemap.xml", "sitemap_index.xml"] {
            let candidate = format!("/{sub_path}/{sitemap_name}");
            if let Ok(sitemap_url) = base.join(&candidate) {
                let sitemap_str = sitemap_url.as_str();
                tracing::debug!("Trying sub-path sitemap: {}", sitemap_str);
                if let Ok(response) = wreq::Client::new().head(sitemap_str).send().await {
                    if response.status().is_success() {
                        tracing::info!("Found sub-path sitemap: {}", sitemap_str);
                        if let Ok(urls) = parser.parse_from_url(sitemap_str).await {
                            tracing::info!(
                                "Parsed {} URLs from sub-path sitemap {}",
                                urls.len(),
                                sitemap_str
                            );
                            all_urls.extend(urls);
                        }
                    }
                }
            }
        }
    }

    if all_urls.is_empty() {
        tracing::warn!("no se encontraron sitemaps de subruta para {}", base_url);
        Ok(Vec::new())
    } else {
        Ok(all_urls
            .into_iter()
            .map(|url| DiscoveredUrl::html(url, 0, base.clone()))
            .collect())
    }
}

/// Auto-discover sitemap URL from robots.txt or fallback
///
/// Following **own-borrow-over-clone**: Accepts `&str`.
/// Following **security-no-unwrap-in-prod**: Proper error handling.
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
///
/// # Returns
///
/// * `Ok(String)` - Discovered sitemap URL
/// * `Err(CrawlError)` - Error during discovery
async fn discover_sitemap_url(base_url: &str) -> Result<String, CrawlError> {
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

    // Try robots.txt first
    let robots_url = base
        .join("/robots.txt")
        .map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

    tracing::info!("Checking robots.txt: {}", robots_url);
    if let Ok(response) = wreq::get(robots_url.as_str()).send().await {
        tracing::info!("robots.txt status: {}", response.status());
        if response.status().is_success() {
            if let Ok(content) = response.text().await {
                tracing::info!(
                    "robots.txt content (first 500 chars):\n{}",
                    &content[..content.len().min(500)]
                );
                // Extract Sitemap: directive
                for line in content.lines() {
                    if line.to_lowercase().starts_with("sitemap:") {
                        if let Some(sitemap) = line
                            .strip_prefix("Sitemap:")
                            .or_else(|| line.strip_prefix("sitemap:"))
                        {
                            let sitemap = sitemap.trim();
                            // Resolve relative URLs from robots.txt against base
                            let resolved = if sitemap.starts_with("http://")
                                || sitemap.starts_with("https://")
                            {
                                Url::parse(sitemap).ok()
                            } else {
                                base.join(sitemap).ok()
                            };
                            if let Some(url) = resolved {
                                tracing::debug!("Found sitemap in robots.txt: {}", url);
                                return Ok(url.to_string());
                            } else {
                                tracing::warn!("Invalid sitemap URL in robots.txt: {}", sitemap);
                            }
                        }
                    }
                }
            }
        }
    }

    tracing::debug!("No sitemap found in robots.txt, trying fallback locations");

    // Fallback: try common sitemap locations
    let fallback_urls = [
        "/sitemap.xml",
        "/sitemap_index.xml",
        "/sitemap.xml.gz",
        "/sitemap/sitemap.xml",
    ];

    for path in &fallback_urls {
        let sitemap_url = base
            .join(path)
            .map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
        let sitemap_str = sitemap_url.as_str();

        // Quick HEAD request to check if exists
        tracing::info!("Trying fallback sitemap: {}", sitemap_str);
        if let Ok(response) = wreq::Client::new().head(sitemap_str).send().await {
            tracing::info!("  Status: {}", response.status());
            if response.status().is_success() {
                tracing::debug!("Found sitemap at fallback location: {}", sitemap_str);
                return Ok(sitemap_str.to_string());
            }
        }
    }

    // GAP 5 (Bug #30): Try sub-path sitemaps for nested sites
    // e.g. https://example.com/docs/en/ → /docs/sitemap.xml, /docs/en/sitemap.xml
    let path = base.path();
    let segments: Vec<_> = path.split('/').filter(|s| !s.is_empty()).collect();
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for sitemap_name in &["sitemap.xml", "sitemap_index.xml"] {
            let candidate = format!("/{sub_path}/{sitemap_name}");
            if let Ok(sitemap_url) = base.join(&candidate) {
                let sitemap_str = sitemap_url.as_str();
                tracing::debug!("Trying sub-path sitemap: {}", sitemap_str);
                if let Ok(response) = wreq::Client::new().head(sitemap_str).send().await {
                    if response.status().is_success() {
                        tracing::info!("Found sitemap at sub-path: {}", sitemap_str);
                        return Ok(sitemap_str.to_string());
                    }
                }
            }
        }
    }
    // No sitemap found - return error instead of guessing
    tracing::warn!("no sitemap found for {}", base_url);
    Err(CrawlError::SitemapNotFound(base_url.to_string()))
}

/// Parse sitemap XML content using quick-xml (streaming parser)
///
/// Following **xml-no-regex**: Uses quick-xml instead of regex for XML parsing.
/// Following **mem-stream-processing**: Streaming approach avoids loading entire DOM.
///
/// # Arguments
///
/// * `xml_content` - XML content of the sitemap
///
/// # Returns
///
/// * `Ok(Vec<String>)` - List of URLs
/// * `Err(CrawlError)` - Parse error
pub fn parse_sitemap(xml_content: &str, base_url: &Url) -> Result<Vec<String>, CrawlError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml_content);
    let mut buf = Vec::new();
    let mut urls = Vec::new();
    let mut in_loc = false;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) if e.name().as_ref() == b"loc" => {
                in_loc = true;
            },
            Ok(Event::End(ref e)) if e.name().as_ref() == b"loc" => {
                in_loc = false;
            },
            Ok(Event::Text(ref e)) if in_loc => {
                let text = e.decode().map_err(|e| CrawlError::Parse(e.to_string()))?;
                let url_str = text.trim();
                if !url_str.is_empty() {
                    // Resolve relative URLs against base_url
                    // Following url-join-relative: use base_url.join() for relative paths
                    let resolved =
                        if url_str.starts_with("http://") || url_str.starts_with("https://") {
                            Url::parse(url_str).ok()
                        } else {
                            base_url.join(url_str).ok()
                        };
                    if let Some(url) = resolved {
                        urls.push(url.to_string());
                    }
                }
            },
            Ok(Event::CData(ref e)) if in_loc => {
                // Handle CDATA sections - BytesCData derefs to [u8]
                let url_str = String::from_utf8_lossy(e).trim().to_string();
                if !url_str.is_empty() {
                    // Resolve relative URLs against base_url
                    let resolved =
                        if url_str.starts_with("http://") || url_str.starts_with("https://") {
                            Url::parse(&url_str).ok()
                        } else {
                            base_url.join(&url_str).ok()
                        };
                    if let Some(url) = resolved {
                        urls.push(url.to_string());
                    }
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(CrawlError::Parse(e.to_string())),
            _ => {},
        }
    }

    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_sitemap_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url>
        <loc>https://example.com/page1</loc>
    </url>
    <url>
        <loc>https://example.com/page2</loc>
    </url>
    <url>
        <loc>https://example.com/page3</loc>
    </url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://example.com/page1");
        assert_eq!(urls[1], "https://example.com/page2");
        assert_eq!(urls[2], "https://example.com/page3");
    }

    #[test]
    fn test_parse_sitemap_with_cdata() {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc><![CDATA[https://example.com/page1]]></loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://example.com/page1".to_string()));
        assert!(urls.contains(&"https://example.com/page2".to_string()));
    }

    #[test]
    fn test_parse_sitemap_with_namespaces() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:xhtml="http://www.w3.org/1999/xhtml">
    <url>
        <loc>https://example.com/page1</loc>
    </url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com/page1");
    }

    #[test]
    fn test_parse_sitemap_xml_empty() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert!(urls.is_empty());
    }

    #[test]
    fn test_parse_sitemap_invalid_xml() {
        // Spec Scenario 9: non-XML content returns Ok with empty vec (graceful degradation)
        let xml = "not xml at all";
        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert!(urls.is_empty());
    }

    #[test]
    fn test_parse_sitemap_relative_urls_resolved() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>/page1</loc></url>
    <url><loc>https://external.com/page2</loc></url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://example.com/page1".to_string()));
        assert!(urls.contains(&"https://external.com/page2".to_string()));
    }
}
