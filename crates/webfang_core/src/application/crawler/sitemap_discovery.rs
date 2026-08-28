//! Sitemap discovery — sitemap-based URL discovery use cases.
//!
//! Hosts the sitemap crawling orchestration: explicit/auto-discovered sitemap
//! fetching, sub-path sitemap fallback, and robots.txt sitemap discovery. These
//! were extracted from `discovery.rs` (issue #442) to separate the sitemap
//! infrastructure concern from DOM-link discovery.

use crate::application::url_filter::is_allowed;
use crate::domain::error::WafDetectionKind;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::waf::{InspectionContext, WafInspector};
use crate::domain::{CrawlError, CrawlerConfig, DiscoveredUrl};
use crate::infrastructure::crawler::{SitemapConfig, SitemapError, SitemapParser, SitemapUrl};
use tracing::{info, instrument};
use url::Url;

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
    // gzip/brotli as the DOM path (#298).
    let discovery_client = build_discovery_client(config)?;

    // Use default batch size (10,000) - SitemapConfig handles pagination
    const DEFAULT_BATCH_SIZE: usize = 10_000;

    // Auto-discover sitemap URL if not provided
    let sitemap_url = resolve_sitemap_url(base_url, sitemap_url, &discovery_client).await?;

    tracing::info!("Using sitemap: {}", sitemap_url);

    // Create sitemap parser with config (including pagination settings).
    // #323: thread the configured TLS/H2 profile so sitemap XML fetches honor
    // the user's --h2-profile selection instead of a hardcoded Chrome145.
    let parser = build_sitemap_parser(config, DEFAULT_BATCH_SIZE)?;

    // Parse sitemap
    let urls = parse_sitemap(&parser, &sitemap_url).await?;

    // Validate sitemap relevance: check if any URLs share a path prefix
    // with the target URL. This handles cases where robots.txt points to
    // an unrelated sitemap (e.g. blog sitemap for a docs site).
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;
    let target_path = base.path().to_string();
    let relevant_urls = filter_relevant_urls(urls, &target_path);

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

    build_discovered_urls_or_limit_error(relevant_urls, &base, config)
}

/// Apply the depth gate to discovered sitemap URLs and surface a crawl-limit
/// error when the configuration makes the result empty.
///
/// `max_depth 0` means "only the seed page": a sitemap that lists the seed
/// itself still yields it (depth 0), but a sitemap listing only deeper URLs
/// yields nothing — that is a configuration limit, not an empty sitemap, so it
/// maps to exit 69 via `MaxDepthExceeded` instead of exit 2
/// (stabilization-sitemap-regression, scenario 8).
fn build_discovered_urls_or_limit_error(
    relevant_urls: Vec<SitemapUrl>,
    base: &Url,
    config: &CrawlerConfig,
) -> Result<Vec<DiscoveredUrl>, CrawlError> {
    let discovered = build_discovered_urls(relevant_urls, base, config);
    if discovered.is_empty() && config.max_depth == 0 {
        return Err(CrawlError::MaxDepthExceeded { current: 1, max: 0 });
    }
    Ok(discovered)
}

/// Build the shared discovery HTTP client with the #281/#312 timeout and TLS
/// profile policy.
fn build_discovery_client(config: &CrawlerConfig) -> Result<wreq::Client, CrawlError> {
    // Timeouts replicate the #281 policy: request timeout as configured,
    // connect timeout capped at 10s.
    let http_config = HttpClientConfig {
        timeout_secs: config.timeout_secs,
        connect_timeout_secs: config.timeout_secs.min(10), // #281 policy
        tls_emulation: config.tls_emulation,               // #312 honor configured profile
        ..Default::default()
    };
    super::super::create_http_client_with_config(&http_config)
        // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
        .map_err(|e| CrawlError::Internal(format!("failed to build discovery client: {e}")))
}

/// Resolve the sitemap URL: use the provided one, or auto-discover it.
async fn resolve_sitemap_url(
    base_url: &str,
    sitemap_url: Option<&str>,
    client: &wreq::Client,
) -> Result<String, CrawlError> {
    match sitemap_url {
        Some(url) if !url.is_empty() => {
            tracing::info!("Sitemap URL provided: {}", url);
            Ok(url.to_string())
        },
        _ => discover_sitemap_url_for(base_url, client).await,
    }
}

/// Auto-discover a sitemap URL, logging the discovery outcome.
async fn discover_sitemap_url_for(
    base_url: &str,
    client: &wreq::Client,
) -> Result<String, CrawlError> {
    tracing::info!("Auto-discovering sitemap URL for {}", base_url);
    match discover_sitemap_url(base_url, client).await {
        Ok(url) => {
            tracing::info!("Discovered sitemap URL: {}", url);
            Ok(url)
        },
        Err(CrawlError::SitemapNotFound(url)) => Err(CrawlError::SitemapNotFound(url)),
        Err(e) => Err(e),
    }
}

/// Build the sitemap parser with pagination settings and the configured TLS
/// profile.
fn build_sitemap_parser(
    config: &CrawlerConfig,
    batch_size: usize,
) -> Result<SitemapParser, CrawlError> {
    SitemapParser::with_config_and_profile(
        SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(3)
            .concurrency(5)
            .batch_size(batch_size)
            .pagination_enabled(true)
            .build(),
        config.tls_emulation,
    )
}

/// Parse a sitemap URL, mapping parse failures to a [`CrawlError`].
async fn parse_sitemap(
    parser: &SitemapParser,
    sitemap_url: &str,
) -> Result<Vec<SitemapUrl>, CrawlError> {
    let urls = parser.parse_from_url(sitemap_url).await.map_err(|e| {
        tracing::error!("Failed to parse sitemap {}: {}", sitemap_url, e);
        // Preserve the specific error type for proper exit code mapping
        match e {
            SitemapError::HttpError { status, message } => CrawlError::Http {
                status,
                url: message,
            },
            SitemapError::XmlError(e) => CrawlError::Parse(format!("XML parsing failed: {e}")),
            SitemapError::InvalidContentType(ct) => CrawlError::InvalidContentType(ct),
            SitemapError::SitemapNotFound(url) => CrawlError::SitemapNotFound(url),
            SitemapError::MaxDepthExceeded => CrawlError::SitemapDepthExceeded,
            SitemapError::NoUrlsFound => CrawlError::SitemapEmpty,
            SitemapError::DecompressionError(e) => {
                CrawlError::Parse(format!("decompression failed: {e}"))
            },
            // All children FAILED to fetch/parse (HTTP errors) — this is an
            // infrastructure failure (exit 69 via Parse→Internal), NOT a
            // fully-empty sitemap (which is NoUrlsFound→SitemapEmpty→exit 2).
            // Keep this mapping.
            SitemapError::AllChildrenFailed(count, details) => {
                CrawlError::Parse(format!("all {count} child sitemaps failed: {details}"))
            },
            // Issue #879: a challenge detected in the sitemap chain keeps its
            // typed identity through the CrawlError layer (PermanentFatal per
            // classify(), Spanish evidence chain via ScraperError::WafBlocked).
            SitemapError::WafChallenge { url, provider } => CrawlError::WafChallenge {
                provider,
                kind: WafDetectionKind::BodySignature,
                url,
            },
            other => CrawlError::Parse(other.to_string()),
        }
    })?;
    tracing::info!("Parsed {} total URLs from sitemap", urls.len());
    Ok(urls)
}

/// Keep only sitemap URLs whose path shares the target's path prefix.
///
/// Paths are compared using the domain's [`canonical_path`] so that a seed of
/// `/docs/` still matches a sitemap URL `/docs` (and vice-versa). Without this,
/// `"/docs".starts_with("/docs/")` is `false` and the section index is silently
/// dropped.
///
/// [`canonical_path`]: crate::domain::url_validation::canonical_path
fn filter_relevant_urls(urls: Vec<SitemapUrl>, target_path: &str) -> Vec<SitemapUrl> {
    let target = crate::domain::url_validation::canonical_path(target_path);
    urls.into_iter()
        .filter(|url| {
            crate::domain::url_validation::canonical_path(url.url.path()).starts_with(target)
        })
        .collect()
}

/// Map relevant sitemap URLs to discovered URLs, applying include/exclude
/// patterns and the depth gate.
///
/// Depth assignment: the seed itself is depth 0; every other sitemap URL is one
/// hop from the seed, hence depth 1. Filtering by `depth <= max_depth` enforces
/// the CLI contract "0 = only seed URL" here, because the CLI scrape flow scrapes
/// whatever discovery returns verbatim — there is no later depth gate (the Engine's
/// `run_crawl_task` check is a separate, non-CLI code path).
fn build_discovered_urls(
    relevant_urls: Vec<SitemapUrl>,
    base: &Url,
    config: &CrawlerConfig,
) -> Vec<DiscoveredUrl> {
    let max_depth = config.max_depth;
    relevant_urls
        .into_iter()
        .filter(|url| is_allowed(url.url.as_str(), config))
        .filter_map(|url| {
            let depth = if url.url == *base { 0 } else { 1 };
            (depth <= max_depth).then(|| DiscoveredUrl::html(url.url, depth, base.clone()))
        })
        .collect()
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

    let all_urls = probe_subpath_sitemaps_for_crawl(base, client, parser).await;

    if all_urls.is_empty() {
        tracing::warn!("no se encontraron sitemaps de subruta para {}", base_url);
        Ok(Vec::new())
    } else {
        // Sub-path sitemap URLs are at depth 1 (one hop from seed)
        Ok(all_urls
            .into_iter()
            .map(|sitemap_url| DiscoveredUrl::html(sitemap_url.url, 1, base.clone()))
            .collect())
    }
}

/// Probe up to 3 sub-path sitemap levels (`/docs`, `/docs/en`, ...) for both
/// `sitemap.xml` and `sitemap_index.xml`, returning every parsed URL.
async fn probe_subpath_sitemaps_for_crawl(
    base: &Url,
    client: &wreq::Client,
    parser: &SitemapParser,
) -> Vec<SitemapUrl> {
    let segments: Vec<_> = base.path().split('/').filter(|s| !s.is_empty()).collect();
    let mut all_urls = Vec::new();

    // Try up to 3 path levels: /docs, /docs/en, /docs/en/quickstart
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for sitemap_name in &["sitemap.xml", "sitemap_index.xml"] {
            let candidate = format!("/{sub_path}/{sitemap_name}");
            if let Some(urls) = try_subpath_sitemap(base, client, parser, &candidate).await {
                all_urls.extend(urls);
            }
        }
    }
    all_urls
}

/// Probe a single sub-path sitemap candidate; returns its parsed URLs when the
/// server answers 2xx, otherwise `None`.
async fn try_subpath_sitemap(
    base: &Url,
    client: &wreq::Client,
    parser: &SitemapParser,
    candidate: &str,
) -> Option<Vec<SitemapUrl>> {
    let sitemap_url = base.join(candidate).ok()?;
    let sitemap_str = sitemap_url.as_str();
    tracing::debug!("Trying sub-path sitemap: {}", sitemap_str);
    let response = client.head(sitemap_str).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    tracing::info!("Found sub-path sitemap: {}", sitemap_str);
    parse_subpath_sitemap(parser, sitemap_str).await
}

/// Parse a discovered sub-path sitemap, logging the URL count on success.
async fn parse_subpath_sitemap(
    parser: &SitemapParser,
    sitemap_str: &str,
) -> Option<Vec<SitemapUrl>> {
    match parser.parse_from_url(sitemap_str).await {
        Ok(urls) => {
            tracing::info!(
                "Parsed {} URLs from sub-path sitemap {}",
                urls.len(),
                sitemap_str
            );
            Some(urls)
        },
        Err(_) => None,
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

    // 1. robots.txt `Sitemap:` directive (a WAF challenge aborts via `?`)
    if let Some(url) = fetch_robots_sitemap(&base, client).await? {
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
///
/// The response body is inspected for WAF challenges before directive
/// parsing (issue #879): a challenge page served where `robots.txt` should
/// be aborts discovery with [`CrawlError::WafChallenge`] instead of being
/// silently treated as "no robots.txt". Every other failure mode keeps the
/// legacy `None` ("not found") semantics.
async fn fetch_robots_sitemap(
    base: &Url,
    client: &wreq::Client,
) -> Result<Option<String>, CrawlError> {
    let Ok(robots_url) = base.join("/robots.txt") else {
        tracing::warn!("invalid base URL for robots.txt: {}", base);
        return Ok(None);
    };
    tracing::info!("Checking robots.txt: {}", robots_url);

    let Ok(response) = client.get(robots_url.as_str()).send().await else {
        return Ok(None);
    };
    let status = response.status();
    tracing::info!("robots.txt status: {}", status);
    if !status.is_success() {
        return Ok(None);
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Ok(content) = response.text().await else {
        return Ok(None);
    };

    // Issue #879: inspect the body before directive parsing — Challenge-tier
    // markers block at any status; Fingerprint-tier evidence never blocks at
    // 2xx (REQ-WAF-09), so benign vendor mentions keep flowing through.
    let ctx = InspectionContext {
        status: Some(status.as_u16()),
        content_type: (!content_type.is_empty()).then_some(content_type),
        headers: std::collections::HashMap::new(),
        ignore_waf: false,
    };
    let verdict = WafInspector::inspect(&content, &ctx);
    if verdict.is_blocked {
        tracing::warn!(
        url = %robots_url,
        status = status.as_u16(),
        evidences = verdict.evidences.len(),
        "WAF/CAPTCHA challenge detected in robots.txt body; aborting sitemap discovery"
                );
        return Err(CrawlError::WafChallenge {
            provider: verdict.evidence_chain(),
            kind: WafDetectionKind::BodySignature,
            url: robots_url.to_string(),
        });
    }

    log_robots_content(&content);
    Ok(extract_robots_sitemap_directive(&content, base))
}

/// Log the robots.txt body (capped to the first 500 chars for instrumentation).
fn log_robots_content(content: &str) {
    tracing::info!(
        "robots.txt content (first 500 chars):\n{}",
        &content[..content.len().min(500)]
    );
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
    probe_head_success(client, sitemap_url.as_str()).await
}

/// HEAD-probe a resolved sitemap URL, returning it when the server answers 2xx.
///
/// Servers that reject HEAD (405/501) or answer any other non-2xx to HEAD while
/// still serving the resource over GET get ONE GET retry: a HEAD-only failure
/// must not silently kill sitemap discovery (stabilization-sitemap-regression,
/// scenario 9).
#[tracing::instrument(skip(client), fields(url = %sitemap_str))]
async fn probe_head_success(client: &wreq::Client, sitemap_str: &str) -> Option<String> {
    tracing::info!("Trying fallback sitemap: {}", sitemap_str);

    let response = client.head(sitemap_str).send().await.ok()?;
    tracing::info!("  Status: {}", response.status());
    if response.status().is_success() {
        tracing::debug!("Found sitemap at fallback location: {}", sitemap_str);
        return Some(sitemap_str.to_string());
    }

    // HEAD was rejected — retry once with GET on the same client.
    tracing::info!(
        url = %sitemap_str,
        status = %response.status(),
        "HEAD rejected, retrying with GET"
    );
    let get_response = client.get(sitemap_str).send().await.ok()?;
    tracing::info!("  GET status: {}", get_response.status());
    if get_response.status().is_success() {
        tracing::debug!("Found sitemap via GET fallback: {}", sitemap_str);
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

// All tests in this module build a wreq::Client (test_client()), which
// depends on boring-sys2 (BoringSSL → TLS_method FFI). Miri cannot execute
// C FFI — gate the whole module instead of patching test by test, keeping
// Miri focused on UB in pure Rust logic (same pattern as readability).
#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::domain::error::WafDetectionKind;
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

    // ── Issue #879 (Option A): WAF inspection over sitemap-chain bodies ──

    /// Cloudflare Turnstile widget marker — Challenge-tier (T1), blocks even
    /// in degraded mode per REQ-WAF-05/09.
    const CHALLENGE_HTML: &str = r#"<html><body>Just a moment...</body><div id="cf-turnstile" data-sitekey="abc"></div></html>"#;

    /// Shared writer capturing tracing fmt output for assertions (same
    /// pattern as `pipeline::executor` span-capture tests).
    #[derive(Clone)]
    struct SharedWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard(self.0.clone())
        }
    }

    struct SharedWriterGuard(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("trace buffer lock")
                .extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// A WAF challenge served where robots.txt should be must abort discovery
    /// with the typed `CrawlError::WafChallenge` (carrying host context and
    /// evidence chain) and emit a structured trace event — not silently fall
    /// through to `SitemapNotFound` (issue #879).
    #[test]
    fn robots_txt_waf_challenge_yields_typed_error_and_trace_event() {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(SharedWriter(buf.clone()))
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime");
            rt.block_on(async {
                let mock = MockServer::start().await;
                Mock::given(method("GET"))
                    .and(path("/robots.txt"))
                    .respond_with(ResponseTemplate::new(200).set_body_string(CHALLENGE_HTML))
                    .mount(&mock)
                    .await;

                let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;

                match result {
                    Err(CrawlError::WafChallenge {
                        provider,
                        kind,
                        url,
                    }) => {
                        assert_eq!(kind, WafDetectionKind::BodySignature);
                        assert!(
                            provider.contains("Cloudflare"),
                            "evidence chain expected in provider, got: {provider}"
                        );
                        assert_eq!(url, format!("{}/robots.txt", mock.uri()));
                    },
                    other => panic!("expected CrawlError::WafChallenge, got: {other:?}"),
                }
            });
        });

        let captured = buf.lock().expect("trace buffer lock").clone();
        let out = String::from_utf8_lossy(&captured);
        assert!(
            out.contains("challenge"),
            "WAF trace event expected on the discovery path, got: {out}"
        );
    }

    /// A benign robots.txt that merely mentions a WAF vendor at status 200 is
    /// Fingerprint-tier evidence, which never blocks without a correlated WAF
    /// status (REQ-WAF-09): discovery must proceed to fallbacks as before.
    #[tokio::test]
    async fn benign_robots_txt_mentioning_vendor_does_not_trigger_waf_error() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/robots.txt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("User-agent: *\n# site hosted behind cloudflare\n"),
            )
            .mount(&mock)
            .await;

        let result = discover_sitemap_url(mock.uri().as_str(), &test_client()).await;
        assert!(
            !matches!(result, Err(CrawlError::WafChallenge { .. })),
            "benign vendor mention must not raise WafChallenge, got: {result:?}"
        );
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
