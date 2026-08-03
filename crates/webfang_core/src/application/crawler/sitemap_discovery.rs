//! Sitemap discovery — sitemap-based URL discovery use cases.
//!
//! Hosts the sitemap crawling orchestration: explicit/auto-discovered sitemap
//! fetching, sub-path sitemap fallback, and robots.txt sitemap discovery. These
//! were extracted from `discovery.rs` (issue #442) to separate the sitemap
//! infrastructure concern from DOM-link discovery.

use tracing::{info, instrument};
use url::Url;

use crate::application::url_filter::is_allowed;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::{CrawlError, CrawlerConfig, DiscoveredUrl};
use crate::infrastructure::crawler::{SitemapConfig, SitemapParser};

/// Crawl site using sitemap (preferred method - FASE 3)
///
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
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let seed = Url::parse("https://example.com")?;
/// let config = CrawlerConfig::new(seed);
///
/// let urls = crawl_with_sitemap("https://example.com", None, &config).await?;
/// println!("Found {} URLs from sitemap", urls.len());
/// # Ok(())
/// # }
/// ```
#[instrument(
    name = "crawl_with_sitemap",
    skip(config),
    fields(
        base_url,
        sitemap_url = ?sitemap_url
    )
)]
pub async fn crawl_with_sitemap(
    base_url: &str,
    sitemap_url: Option<&str>,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    crawl_with_sitemap_internal(base_url, sitemap_url, config).await
}

/// Crawl with sitemap (internal version with progress tracking)
///
/// This is the internal implementation that supports optional progress tracking.
/// The public `crawl_with_sitemap` function calls this one.
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
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
