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
        // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
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

/// Auto-discover sitemap URL from robots.txt or fallback locations.
///
/// Resolution order (each step short-circuits on first hit):
/// 1. Parse the `Sitemap:` directive from `robots.txt` (resolved against `base`).
/// 2. HEAD-probe common fallback paths (`/sitemap.xml`, ...).
/// 3. HEAD-probe nested sub-path sitemaps for sectioned sites (GAP 5 / Bug #30).
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

    // 1. robots.txt `Sitemap:` directive
    if let Some(url) = fetch_robots_sitemap(&base, client).await {
        return Ok(url);
    }
    tracing::debug!("No sitemap found in robots.txt, trying fallback locations");

    // 2. Common fallback locations
    const FALLBACK_PATHS: &[&str] = &[
        "/sitemap.xml",
        "/sitemap_index.xml",
        "/sitemap.xml.gz",
        "/sitemap/sitemap.xml",
    ];
    if let Some(url) = probe_sitemap_paths(&base, client, FALLBACK_PATHS).await {
        return Ok(url);
    }

    // 3. Nested sub-path sitemaps for sectioned sites
    if let Some(url) = probe_subpath_sitemaps(&base, client).await {
        return Ok(url);
    }

    tracing::warn!("no sitemap found for {}", base_url);
    Err(CrawlError::SitemapNotFound(base_url.to_string()))
}

/// Fetch `robots.txt` and extract its `Sitemap:` directive, if present.
async fn fetch_robots_sitemap(base: &Url, client: &wreq::Client) -> Option<String> {
    let robots_url = base.join("/robots.txt").ok()?;
    tracing::info!("Checking robots.txt: {}", robots_url);

    let response = client.get(robots_url.as_str()).send().await.ok()?;
    tracing::info!("robots.txt status: {}", response.status());
    if !response.status().is_success() {
        return None;
    }

    let content = response.text().await.ok()?;
    tracing::info!(
        "robots.txt content (first 500 chars):\n{}",
        &content[..content.len().min(500)]
    );
    extract_robots_sitemap_directive(&content, base)
}

/// Parse the first `Sitemap:` directive from a `robots.txt` body and resolve it
/// against `base` (absolute `http(s)` URLs are parsed directly).
fn extract_robots_sitemap_directive(content: &str, base: &Url) -> Option<String> {
    for line in content.lines() {
        if !line.to_lowercase().starts_with("sitemap:") {
            continue;
        }
        let Some(directive) = line
            .strip_prefix("Sitemap:")
            .or_else(|| line.strip_prefix("sitemap:"))
        else {
            continue;
        };
        let sitemap = directive.trim();

        let resolved = if sitemap.starts_with("http://") || sitemap.starts_with("https://") {
            Url::parse(sitemap).ok()
        } else {
            base.join(sitemap).ok()
        };

        match resolved {
            Some(url) => {
                tracing::debug!("Found sitemap in robots.txt: {}", url);
                return Some(url.to_string());
            },
            None => tracing::warn!("Invalid sitemap URL in robots.txt: {}", sitemap),
        }
    }
    None
}

/// HEAD-probe each candidate path and return the first that responds 2xx.
async fn probe_sitemap_paths(base: &Url, client: &wreq::Client, paths: &[&str]) -> Option<String> {
    for path in paths {
        if let Some(found) = probe_single_path(base, client, path).await {
            return Some(found);
        }
    }
    None
}

/// HEAD-probe a single candidate path; returns its URL when the server answers
/// 2xx, otherwise `None`.
async fn probe_single_path(base: &Url, client: &wreq::Client, path: &str) -> Option<String> {
    let Ok(sitemap_url) = base.join(path) else {
        tracing::warn!("Invalid sitemap candidate path: {path}");
        return None;
    };
    let sitemap_str = sitemap_url.as_str();
    tracing::info!("Trying fallback sitemap: {}", sitemap_str);

    let Ok(response) = client.head(sitemap_str).send().await else {
        return None;
    };
    tracing::info!("  Status: {}", response.status());
    if response.status().is_success() {
        tracing::debug!("Found sitemap at fallback location: {}", sitemap_str);
        return Some(sitemap_str.to_string());
    }
    None
}

/// Build nested sub-path sitemap candidates (e.g. `/docs/sitemap.xml`) and probe
/// them, honoring the same 3-level depth cap as `crawl_with_subpath_sitemaps`.
async fn probe_subpath_sitemaps(base: &Url, client: &wreq::Client) -> Option<String> {
    let segments: Vec<_> = base.path().split('/').filter(|s| !s.is_empty()).collect();
    let mut candidates = Vec::with_capacity(segments.len().min(3) * 2);
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for name in ["sitemap.xml", "sitemap_index.xml"] {
            candidates.push(format!("/{sub_path}/{name}"));
        }
    }
    let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    probe_sitemap_paths(base, client, &refs).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_client() -> wreq::Client {
        wreq::Client::new()
    }

    #[tokio::test]
    async fn discovers_via_robots_absolute_directive() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\nSitemap: https://example.com/sitemap.xml\n"),
            )
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert_eq!(result.unwrap(), "https://example.com/sitemap.xml");
    }

    #[tokio::test]
    async fn discovers_via_robots_relative_directive() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\nSitemap: /sitemap.xml\n"),
            )
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert_eq!(result.unwrap(), format!("{}/sitemap.xml", mock.uri()));
    }

    #[tokio::test]
    async fn discovers_via_fallback_location() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert_eq!(result.unwrap(), format!("{}/sitemap.xml", mock.uri()));
    }

    #[tokio::test]
    async fn discovers_via_subpath_sitemap() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/docs/sitemap.xml"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&mock)
            .await;

        let base = format!("{}/docs", mock.uri());
        let result = discover_sitemap_url(&base, &test_client()).await;
        assert_eq!(result.unwrap(), format!("{}/docs/sitemap.xml", mock.uri()));
    }

    #[tokio::test]
    async fn errors_when_no_sitemap_found() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;
        Mock::given(method("HEAD"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert!(result.is_err());
    }
}
