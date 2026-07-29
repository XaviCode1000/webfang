//! Discovery module — URL discovery and sitemap parsing
//!
//! Functions for discovering URLs from websites via sitemaps or DOM scraping.
//! Part of the TUI workflow: discover → select → scrape.

use anyhow::Result;
use tracing::{debug, info, instrument, span, warn, Level};
use url::Url;

use crate::application::url_filter::is_allowed;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::{
    CorrelationId, CrawlError, CrawlerConfig, DiscoveredUrl, ScrapedContent, ValidUrl,
};
use crate::error::{Result as ScraperResult, ScraperError};
use crate::infrastructure::crawler::binary_utils::derive_filename_from_response;
use crate::infrastructure::crawler::{
    extract_links, is_internal_link, normalize_url, SitemapConfig, SitemapParser,
};
use crate::infrastructure::downloader::{DownloadError, Downloader};
use crate::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
use crate::infrastructure::observability::log_scrape_error;
use crate::infrastructure::scraper::{fallback, readability};
use crate::ScraperConfig;

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;
#[cfg(feature = "adaptive-selectors")]
use crate::domain::ExtractResult;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

// ============================================================================
// TUI Support — Discover/Scrape Use Cases
// ============================================================================

/// Discover URLs from a website without downloading content
///
/// This is the first step in the TUI workflow:
/// 1. Discover all URLs from sitemap or DOM scraping
/// 2. Return `Vec<Url>` for interactive selection
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
/// use webfang_core::{application::discover_urls_for_tui, domain::CrawlerConfig};
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

        Ok(urls)
    } else {
        // DOM scraping - extract links from single page.
        // Honor the configured request timeout (#289). The connect timeout is
        // capped at 10s, replicating the #281 policy used by the sitemap branch.
        // user_agent stays None (Default) so this path keeps rotating random pool
        // agents instead of injecting config.user_agent.
        let http_config = HttpClientConfig {
            timeout_secs: config.timeout_secs,
            connect_timeout_secs: config.timeout_secs.min(10), // #281 policy, replicated at call site
            tls_emulation: config.tls_emulation,               // #312 honor configured profile
            ..Default::default()
        };
        let client = super::super::create_http_client_with_config(&http_config)?;

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
            let normalized = normalize_url(&link, true);
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

        Ok(urls)
    }
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
    url: &Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
) -> ScraperResult<ScrapedContent> {
    // Clean HTML boilerplate (scripts, styles, nav, sidebar, footer) BEFORE
    // Readability. This helps legible find the main content without being
    // confused by navigation elements, JavaScript bundles, and CSS.
    let cleaned_html = crate::infrastructure::converter::html_cleaner::clean_html(html);

    // Apply CSS selector extraction if a non-default selector is configured.
    let extract_result = crate::application::scraper_service::extract_with_selector(
        &cleaned_html,
        &config.selector,
        None,
    );

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
                    let repaired = crate::application::scraper_service::extract_with_selector(
                        &html,
                        &outcome.suggestion.selector,
                        None,
                    );
                    if repaired.is_matched() {
                        info!(
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
            let assets = crate::application::scraper_service::download_assets_if_enabled(
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

            let assets = crate::application::scraper_service::download_assets_if_enabled(
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
    skip(downloader, config, asset_downloader, engine),
    fields(url = %url)
)]
pub async fn scrape_single_url_for_tui(
    downloader: &dyn Downloader,
    url: &Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
) -> ScraperResult<ScrapedContent> {
    let span = span!(Level::DEBUG, "scrape_single", url = %url);
    let _guard = span.enter();

    debug!("Scraping: {}", url);

    // Fetch the page through the injected downloader (a `FetchRouter` in
    // production, a mock in tests). Download-level failures are mapped to their
    // `ScraperError` equivalents so WAF/HTTP semantics survive the conversion.
    let page = downloader.fetch(url).await.map_err(|e| match e {
        DownloadError::WafChallenge(provider) => ScraperError::WafBlocked {
            url: url.to_string(),
            provider,
        },
        DownloadError::Http { status, .. } => ScraperError::http(status, url.as_str()),
        other => ScraperError::Network(Box::new(other)),
    })?;

    if !(200..300).contains(&page.status) {
        return Err(ScraperError::http(page.status, url.as_str()));
    }

    // Check content-type before reading body to handle binary content (PDFs, etc.)
    let content_type = page
        .headers
        .get("content-type")
        .cloned()
        .unwrap_or_default();

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
            let header_map = headers_to_header_map(&page.headers);
            let filename = derive_filename_from_response(&header_map, url, &content_type);
            let output_path = config.output_dir.join(&filename);

            let bytes = page.html.as_bytes();
            if let Err(e) = std::fs::create_dir_all(&config.output_dir) {
                warn!(
                    "Failed to create output directory {}: {}",
                    config.output_dir.display(),
                    e
                );
            } else if let Err(e) = std::fs::write(&output_path, bytes) {
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
        } else {
            None
        };

        let assets = crate::application::scraper_service::download_assets_if_enabled(
            "",
            url,
            config,
            asset_downloader,
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
            correlation_id: Some(CorrelationId::new()),
        });
    }

    let html = page.html;

    // Detect WAF/CAPTCHA challenges disguised as HTTP 200 (H3 fix, REQ-WAF-05).
    // Context-aware inspection keeps the silent-challenge intent: a 200+HTML
    // script-dense body is still caught via the entropy rule (REQ-WAF-06), while
    // bare vendor names at 200 no longer block. A block carries the full Spanish
    // evidence chain (REQ-WAF-08). `ignore_waf` is wired to config in TASK-13.
    let ctx = InspectionContext::from_lowercase_headers(page.status, &page.headers, false);
    let verdict = WafInspector::inspect(&html, &ctx);
    if verdict.is_blocked {
        warn!(
            "WAF challenge detected from {}: {} evidences",
            url,
            verdict.evidences.len()
        );
        return Err(ScraperError::waf_blocked(
            url.to_string(),
            verdict.evidence_chain(),
        ));
    }

    extract_content(&html, url, config, asset_downloader, engine).await
}

/// Convert lowercased string headers into a wreq [`wreq::header::HeaderMap`]
/// for helpers that expect the native header type (e.g.
/// [`derive_filename_from_response`]).
///
/// Invalid header names/values are skipped. They cannot occur for headers
/// captured by the downloaders (already validated via `to_str`), but the guard
/// keeps this conversion infallible.
fn headers_to_header_map(
    headers: &std::collections::HashMap<String, String>,
) -> wreq::header::HeaderMap {
    let mut map = wreq::header::HeaderMap::new();
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            wreq::header::HeaderName::from_bytes(name.as_bytes()),
            wreq::header::HeaderValue::from_str(value),
        ) {
            map.insert(name, value);
        }
    }
    map
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
/// use webfang_core::application::crawl_with_sitemap;
/// use webfang_core::domain::CrawlerConfig;
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

    // Build the discovery client through the shared factory so sitemap probes
    // carry the same Chrome Client Hints, pooled user-agent, pool tuning, and
    // gzip/brotli as the DOM path (#298). Timeouts replicate the #281 policy:
    // request timeout as configured, connect timeout capped at 10s.
    let http_config = HttpClientConfig {
        timeout_secs: config.timeout_secs,
        connect_timeout_secs: config.timeout_secs.min(10), // #281 policy
        tls_emulation: config.tls_emulation,               // #312 honor configured profile
        ..Default::default()
    };
    let discovery_client = super::super::create_http_client_with_config(&http_config)
        .map_err(|e| CrawlError::Internal(format!("failed to build discovery client: {e}")))?;

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
            match discover_sitemap_url(base_url, &discovery_client).await {
                Ok(url) => {
                    tracing::info!("Discovered sitemap URL: {}", url);
                    url
                },
                Err(CrawlError::SitemapNotFound(url)) => {
                    return Err(CrawlError::SitemapNotFound(url));
                },
                Err(e) => return Err(e),
            }
        },
    };

    tracing::info!("Using sitemap: {}", sitemap_url);

    // Create sitemap parser with config (including pagination settings)
    // Following api-builder-pattern: builder API
    // #323: thread the configured TLS/H2 profile so sitemap XML fetches honor
    // the user's --h2-profile selection instead of a hardcoded Chrome145.
    let parser = SitemapParser::with_config_and_profile(
        SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(3)
            .concurrency(5)
            .batch_size(DEFAULT_BATCH_SIZE)
            .pagination_enabled(true)
            .build(),
        config.tls_emulation,
    )?;

    // Parse sitemap
    let urls = parser.parse_from_url(&sitemap_url).await.map_err(|e| {
        tracing::error!("Failed to parse sitemap {}: {}", sitemap_url, e);
        CrawlError::Parse(e.to_string())
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
        return crawl_with_subpath_sitemaps(
            base_url,
            &base,
            &parser,
            3,
            0,
            config.max_depth,
            &discovery_client,
        )
        .await;
    }

    // Following own-borrow-over-clone: use Url directly, not String
    // Apply include/exclude patterns from config (Fix: sitemap URLs were bypassing filters).
    //
    // Depth assignment: the seed itself is depth 0; every other sitemap URL is one
    // hop from the seed, hence depth 1. Filtering by `depth <= max_depth` enforces
    // the CLI contract "0 = only seed URL" here, because the CLI scrape flow scrapes
    // whatever discovery returns verbatim — there is no later depth gate (the Engine's
    // `run_crawl_task` check is a separate, non-CLI code path).
    let max_depth = config.max_depth;
    let discovered: Vec<DiscoveredUrl> = relevant_urls
        .into_iter()
        .filter(|url| is_allowed(url.as_str(), config))
        .filter_map(|url| {
            let depth = if url == base { 0 } else { 1 };
            (depth <= max_depth).then(|| DiscoveredUrl::html(url, depth, base.clone()))
        })
        .collect();

    Ok(discovered)
}

/// Try sub-path sitemaps when the discovered sitemap has no relevant URLs
///
/// For nested sites like `https://example.com/docs/en/`, this tries
/// `/docs/sitemap.xml`, `/docs/en/sitemap.xml`, etc.
/// Follows nested sitemaps recursively up to `sitemap_max_depth` levels.
///
/// `crawl_max_depth` is the crawl's configured max depth (CLI `--max-depth`),
/// distinct from the sitemap-index recursion depth above. Sub-path sitemap URLs
/// are one hop from the seed (depth 1), so when `crawl_max_depth` is 0 ("only
/// the seed URL") none of them qualify and an empty list is returned — mirroring
/// the `depth <= max_depth` gate in `crawl_with_sitemap_internal`.
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
/// Following **err-no-unwrap-prod**: Proper error handling throughout.
async fn crawl_with_subpath_sitemaps(
    base_url: &str,
    base: &Url,
    parser: &SitemapParser,
    sitemap_max_depth: usize,
    sitemap_current_depth: usize,
    crawl_max_depth: u8,
    client: &wreq::Client,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    if sitemap_current_depth >= sitemap_max_depth {
        tracing::warn!(
            "sitemap recursion depth {} reached max {}, stopping",
            sitemap_current_depth,
            sitemap_max_depth
        );
        return Ok(Vec::new());
    }

    // Sub-path sitemap URLs are depth 1; with a crawl max_depth of 0 none pass
    // the gate, so short-circuit before probing the network.
    if crawl_max_depth == 0 {
        tracing::info!(
            "crawl max_depth is 0, skipping sub-path sitemap URLs for {}",
            base_url
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
                if let Ok(response) = client.head(sitemap_str).send().await {
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
        // Sub-path sitemap URLs are at depth 1 (one hop from seed)
        Ok(all_urls
            .into_iter()
            .map(|url| DiscoveredUrl::html(url, 1, base.clone()))
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
async fn discover_sitemap_url(base_url: &str, client: &wreq::Client) -> Result<String, CrawlError> {
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

    // Try robots.txt first
    let robots_url = base
        .join("/robots.txt")
        .map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

    tracing::info!("Checking robots.txt: {}", robots_url);
    if let Ok(response) = client.get(robots_url.as_str()).send().await {
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
        if let Ok(response) = client.head(sitemap_str).send().await {
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
                if let Ok(response) = client.head(sitemap_str).send().await {
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
    use std::time::Duration;

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

    // #289 acceptance tests: the TUI DOM discovery path must honor CrawlerConfig
    // timeouts. Both build a real wreq client (boring-sys2 FFI), hence not(miri).

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_for_tui_respects_request_timeout() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // wiremock completes TCP instantly then delays the HTTP response, so this
        // exercises the request timeout (timeout_secs), not the connect timeout.
        let server = MockServer::start().await;
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html></html>")
                    .set_delay(Duration::from_secs(10)),
            )
            .mount(&server)
            .await;

        let seed = Url::parse(&server.uri()).unwrap();
        let config = CrawlerConfig::builder(seed)
            .timeout_secs(2)
            .use_sitemap(false)
            .build();

        let start = std::time::Instant::now();
        let result = discover_urls_for_tui(&server.uri(), &config).await;
        let elapsed = start.elapsed();

        let err = result.expect_err("slow response should time out");
        assert!(
            elapsed < Duration::from_secs(6),
            "2s request timeout should fire well before 6s, took {elapsed:?}"
        );
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("timed out") || msg.contains("timeout"),
            "expected timeout message, got: {msg}"
        );
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_for_tui_respects_connect_timeout() {
        use tokio::net::TcpListener;

        // TLS blackhole: accept TCP connections and hold them open without ever
        // completing the TLS handshake. wiremock cannot simulate this (it always
        // finishes TCP+TLS), so a raw listener is used. This exercises the connect
        // timeout, which covers TCP+TLS establishment (reqwest semantics).
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                // Keep the socket alive and silent so the handshake never finishes.
                tokio::spawn(async move {
                    let _socket = socket;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });

        let target = format!("https://127.0.0.1:{port}");
        let seed = Url::parse(&target).unwrap();
        let config = CrawlerConfig::builder(seed)
            .timeout_secs(2)
            .use_sitemap(false)
            .build();

        let start = std::time::Instant::now();
        let result = discover_urls_for_tui(&target, &config).await;
        let elapsed = start.elapsed();

        let err = result.expect_err("TLS blackhole should fail to connect");
        assert!(
            elapsed < Duration::from_secs(6),
            "2s connect timeout should fire well before 6s, took {elapsed:?}"
        );
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("client error (connect)")
                || msg.contains("connection")
                || msg.contains("connect"),
            "expected connect-failure message, got: {msg}"
        );
    }

    // ========================================================================
    // scrape_single_url_for_tui — Downloader-injection unit tests (#303)
    // ========================================================================

    use crate::infrastructure::downloader::FetchedPage;
    use futures::future::BoxFuture;
    use std::collections::HashMap;

    struct StubDownloader {
        result: std::sync::Mutex<Option<Result<FetchedPage, DownloadError>>>,
    }

    impl StubDownloader {
        fn returning(result: Result<FetchedPage, DownloadError>) -> Self {
            Self {
                result: std::sync::Mutex::new(Some(result)),
            }
        }
    }

    impl Downloader for StubDownloader {
        fn fetch<'a>(&'a self, _url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
            let result = self
                .result
                .lock()
                .expect("stub lock poisoned")
                .take()
                .expect("fetch called more than once on StubDownloader");
            Box::pin(async move { result })
        }

        fn supports_interactions(&self) -> bool {
            false
        }

        fn memory_cost(&self) -> usize {
            0
        }
    }

    fn stub_page(html: &str, status: u16, headers: Vec<(&str, &str)>) -> FetchedPage {
        FetchedPage {
            url: Url::parse("https://example.com").expect("valid test URL"),
            html: html.to_owned(),
            status,
            headers: headers
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect::<HashMap<_, _>>(),
            cookies: Vec::new(),
        }
    }

    #[tokio::test]
    async fn scrape_single_binary_content_type_returns_early() {
        let page = stub_page(
            "%PDF-1.4 fake bytes",
            200,
            vec![("content-type", "application/pdf")],
        );
        let dl = StubDownloader::returning(Ok(page));
        let url = Url::parse("https://example.com/doc.pdf").expect("valid URL");
        let config = ScraperConfig::new();

        let result = scrape_single_url_for_tui(&dl, &url, &config, None, None)
            .await
            .expect("binary detection should succeed");

        assert!(
            result.content.contains("[Binary content:"),
            "expected binary marker, got: {}",
            result.content
        );
        assert_eq!(result.title, "example.com");
    }

    #[tokio::test]
    async fn scrape_single_waf_body_signature_returns_waf_blocked() {
        let waf_html = r#"<html><body><div id="cf-browser-verification">Checking your browser before accessing the site.</div></body></html>"#;
        let page = stub_page(waf_html, 200, vec![("content-type", "text/html")]);
        let dl = StubDownloader::returning(Ok(page));
        let url = Url::parse("https://example.com").expect("valid URL");
        let config = ScraperConfig::new();

        let err = scrape_single_url_for_tui(&dl, &url, &config, None, None)
            .await
            .expect_err("WAF body should trigger WafBlocked");

        assert!(
            matches!(err, ScraperError::WafBlocked { .. }),
            "expected WafBlocked, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn scrape_single_normal_html_extracts_title_and_content() {
        let html = r#"<html><head><title>Test Article</title></head>
<body><article><h1>Heading</h1>
<p>This is a substantial paragraph with enough text for the readability
algorithm to identify it as the main content of the page and extract it
properly into the ScrapedContent output structure.</p>
<p>A second paragraph ensures the extractor has multiple text blocks to
work with when computing the document readability score.</p>
</article></body></html>"#;
        let page = stub_page(html, 200, vec![("content-type", "text/html")]);
        let dl = StubDownloader::returning(Ok(page));
        let url = Url::parse("https://example.com/article").expect("valid URL");
        let config = ScraperConfig::new();

        let result = scrape_single_url_for_tui(&dl, &url, &config, None, None)
            .await
            .expect("normal HTML should scrape successfully");

        assert!(
            !result.title.is_empty(),
            "title should be extracted from HTML"
        );
        assert!(
            !result.content.is_empty(),
            "content should be extracted from HTML"
        );
    }

    #[tokio::test]
    async fn scrape_single_download_waf_challenge_maps_to_waf_blocked() {
        let dl =
            StubDownloader::returning(Err(DownloadError::WafChallenge("Cloudflare".to_owned())));
        let url = Url::parse("https://example.com").expect("valid URL");
        let config = ScraperConfig::new();

        let err = scrape_single_url_for_tui(&dl, &url, &config, None, None)
            .await
            .expect_err("WafChallenge download error should propagate");

        match err {
            ScraperError::WafBlocked { url, provider } => {
                assert_eq!(provider, "Cloudflare");
                assert!(url.contains("example.com"));
            },
            other => panic!("expected WafBlocked, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn scrape_single_non_2xx_status_returns_http_error() {
        let page = stub_page("not found", 404, vec![("content-type", "text/html")]);
        let dl = StubDownloader::returning(Ok(page));
        let url = Url::parse("https://example.com/missing").expect("valid URL");
        let config = ScraperConfig::new();

        let err = scrape_single_url_for_tui(&dl, &url, &config, None, None)
            .await
            .expect_err("404 status should produce an error");

        assert!(
            matches!(err, ScraperError::Http { status: 404, .. }),
            "expected Http(404), got: {err:?}"
        );
    }

    // ─── extract_content direct tests (no Downloader needed) ───

    #[tokio::test]
    async fn extract_content_readability_extracts_article() {
        let html = r#"<html><head><title>Test Page</title></head><body>
            <article><h1>Hello</h1><p>This is a long enough paragraph of content that readability should be able to extract properly from the DOM structure.</p>
            <p>Second paragraph with more content to ensure readability has enough material to work with for extraction.</p></article>
        </body></html>"#;
        let url = Url::parse("https://example.com/article").unwrap();
        let config = ScraperConfig::default();

        let result = extract_content(html, &url, &config, None, None).await;

        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(!content.content.is_empty());
        assert_eq!(content.url.as_str(), "https://example.com/article");
    }

    #[tokio::test]
    async fn extract_content_fallback_short_content_returns_error() {
        let html = "<html><body><div></div></body></html>";
        let url = Url::parse("https://example.com/tiny").unwrap();
        let config = ScraperConfig::default();

        let result = extract_content(html, &url, &config, None, None).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ScraperError::ExtractionFailed { .. }),
            "Expected ExtractionFailed, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn extract_content_with_selector_extracts_matching_content() {
        let html = r#"<html><body>
            <nav>Navigation stuff</nav>
            <div class="main-content"><p>This is the main content that should be extracted by the CSS selector we configured.</p></div>
            <footer>Footer stuff</footer>
        </body></html>"#;
        let url = Url::parse("https://example.com/page").unwrap();
        let config = ScraperConfig {
            selector: ".main-content".to_string(),
            ..Default::default()
        };

        let result = extract_content(html, &url, &config, None, None).await;

        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.content.contains("main content"));
    }

    #[tokio::test]
    async fn extract_content_populates_correlation_id_natively() {
        // Fase 1 (issue #356): correlation_id must be populated natively,
        // WITHOUT requiring the `otel` feature. Every scraped page must be
        // correlatable with its own logs/traces out of the box.
        let html = r#"<html><head><title>Test Page</title></head><body>
            <article><h1>Hello</h1><p>This is a long enough paragraph of content that readability should be able to extract properly from the DOM structure.</p>
            <p>Second paragraph with more content to ensure readability has enough material to work with for extraction.</p></article>
        </body></html>"#;
        let url = Url::parse("https://example.com/article").unwrap();
        let config = ScraperConfig::default();

        let result = extract_content(html, &url, &config, None, None).await;

        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(
            content.correlation_id.is_some(),
            "correlation_id must be populated natively (without the `otel` feature)"
        );
        // Must be a valid W3C traceparent: 00-{trace_id}-{span_id}-01
        let corr = content.correlation_id.unwrap();
        assert!(corr.to_traceparent().starts_with("00-"));
    }
}
