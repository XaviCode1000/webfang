//! Discovery module — URL discovery and single-URL scraping
//!
//! Functions for discovering URLs from websites (DOM link extraction) and the
//! TUI single-URL scraping use case. Part of the TUI workflow:
//! discover → select → scrape. Sitemap crawling lives in `sitemap_discovery.rs`
//! and sitemap XML parsing in the infrastructure layer (issue #442); both are
//! re-exported here so existing import paths keep resolving.

use tracing::{debug, info, instrument, warn};
use url::Url;

use crate::application::url_filter::is_allowed;
use crate::domain::http_config::HttpClientConfig;
use crate::domain::{CorrelationId, CrawlerConfig, ScrapedContent, ValidUrl};
use crate::error::{Result as ScraperResult, ScraperError};
use crate::infrastructure::crawler::binary_utils::derive_filename_from_response;
use crate::infrastructure::crawler::{extract_links, is_internal_link, normalize_url};
use crate::infrastructure::downloader::{DownloadError, Downloader};
use crate::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
use crate::infrastructure::observability::log_scrape_error;
use crate::ScraperConfig;

#[cfg(feature = "adaptive-selectors")]
use crate::application::adaptive_engine::AdaptiveSelectorEngine;

/// Placeholder when `adaptive-selectors` feature is disabled.
#[cfg(not(feature = "adaptive-selectors"))]
type AdaptiveSelectorEngine = ();

// Sitemap discovery was extracted to `sitemap_discovery.rs` and sitemap XML
// parsing moved to the infrastructure layer (#442). Both are re-exported here so
// `discovery::crawl_with_sitemap` / `discovery::parse_sitemap` (and the `crawler`
// facade that imports them from this module) keep resolving unchanged.
pub use crate::application::crawler::sitemap_discovery::crawl_with_sitemap;
pub use crate::application::extraction::extract_content;
pub use crate::infrastructure::crawler::parse_sitemap;

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
///
/// # Arguments
///
/// * `base_url` - Base URL to discover from
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(Vec<Url>)` - Discovered URLs (owned)
/// * `Err(ScraperError)` - Error during discovery
///
/// # Examples
///
/// ```no_run
/// use webfang_core::{application::discover_urls_for_tui, domain::CrawlerConfig};
/// use url::Url;
///
/// # #[tokio::main]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
) -> ScraperResult<Vec<Url>> {
    info!("Discovering URLs from {}", base_url);

    // If sitemap enabled, use sitemap (preferred)
    if config.use_sitemap {
        let discovered =
            crawl_with_sitemap(base_url, config.sitemap_url.as_deref(), config).await?;
        let urls: Vec<Url> = discovered.into_iter().map(|d| d.url).collect();

        Ok(urls)
    } else if config.max_depth == 0 {
        // Depth 0 means "only seed URL" — skip link extraction entirely.
        // `plan_urls` injects the seed URL in the DOM branch, so returning empty
        // here yields exactly one page crawled (issue #583).
        info!("max_depth is 0 — skipping DOM link extraction, crawling seed only");
        Ok(Vec::new())
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
        let response = client.get(base_url).send().await?;

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

        let html = response.text().await?;

        debug!("Received HTML: {} bytes", html.len());

        let base = Url::parse(base_url)?;

        // Extract links
        let links = extract_links(&html, base_url)?;

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

/// Scrape a single URL
///
/// Following **own-borrow-over-clone**: Accepts `&Url` not `&String`.
///
/// # Arguments
///
/// * `downloader` - Downloader (fetch router in production, mock in tests)
/// * `url` - URL to scrape
/// * `config` - Scraper configuration
/// * `correlation` - Per-page correlation identity, derived by the caller from
///   the run-root identity (`root.child()` — same `trace_id`, fresh
///   `span_id`), so every page of a run stays reconstructable under one trace
///   (#501)
///
/// # Returns
///
/// * `Ok(ScrapedContent)` - Scraped content from the URL
/// * `Err(ScraperError)` - Error during scraping
#[instrument(
    name = "scrape_single_url",
    skip(downloader, config, asset_downloader, engine, binary_writer, correlation),
    fields(url = %url)
)]
pub async fn scrape_single_url_for_tui(
    downloader: &dyn Downloader,
    url: &Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    binary_writer: Option<&dyn crate::domain::ports::BinaryWriterPort>,
    correlation: &CorrelationId,
) -> ScraperResult<ScrapedContent> {
    scrape_single_url_for_tui_inner(
        downloader,
        url,
        config,
        asset_downloader,
        engine,
        binary_writer,
        correlation.clone(),
    )
    .await
}

/// Inner implementation of [`scrape_single_url_for_tui`].
///
/// The `#[instrument]` span declares the per-page identity (`correlation_id`,
/// `trace_id`) AT CREATION time (#501): FileTraceLayer snapshots span fields
/// in `on_new_span`, so fields recorded later never reach the `--trace-file`
/// JSONL. The instrumented span lifecycle is also async-safe — no `enter()`
/// guard crosses an `.await` (#501 follow-up).
#[instrument(
    level = "debug",
    name = "scrape_single",
    skip(downloader, config, asset_downloader, engine, binary_writer, correlation),
    fields(
        url = %url,
        correlation_id = %correlation,
        trace_id = %correlation.trace_id()
    )
)]
async fn scrape_single_url_for_tui_inner(
    downloader: &dyn Downloader,
    url: &Url,
    config: &ScraperConfig,
    asset_downloader: Option<&dyn crate::domain::ports::AssetDownloaderPort>,
    #[allow(unused_variables)] engine: Option<&AdaptiveSelectorEngine>,
    binary_writer: Option<&dyn crate::domain::ports::BinaryWriterPort>,
    correlation: CorrelationId,
) -> ScraperResult<ScrapedContent> {
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

        // Save binary file when download_documents is enabled. Filesystem I/O is
        // routed through the injected BinaryWriterPort (or the FsBinaryWriter
        // fallback) so the application layer never touches std::fs directly
        // (#442 layer-violation fix). Observable behavior is unchanged: the same
        // bytes land in the same file under `config.output_dir`.
        let saved_path = if config.download_documents {
            let header_map = headers_to_header_map(&page.headers);
            let filename = derive_filename_from_response(&header_map, url, &content_type);
            let output_path = config.output_dir.join(&filename);

            let bytes = page.html.as_bytes();
            let fallback_writer = crate::infrastructure::crawler::FsBinaryWriter::new();
            let writer: &dyn crate::domain::ports::BinaryWriterPort =
                binary_writer.unwrap_or(&fallback_writer);
            match writer.write_bytes(&output_path, bytes) {
                Ok(()) => info!(
                    "Saved binary file: {} ({} bytes)",
                    output_path.display(),
                    bytes.len()
                ),
                Err(e) => warn!(
                    "Failed to save binary file {}: {}",
                    output_path.display(),
                    e
                ),
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
            correlation_id: Some(correlation),
        });
    }

    let html = page.html;

    // Detect WAF/CAPTCHA challenges disguised as HTTP 200 (H3 fix, REQ-WAF-05).
    // Context-aware inspection keeps the silent-challenge intent: a 200+HTML
    // script-dense body is still caught via the entropy rule (REQ-WAF-06), while
    // bare vendor names at 200 no longer block. A block carries the full Spanish
    // evidence chain (REQ-WAF-08). `config.ignore_waf` short-circuits to a clean
    // verdict (REQ-WAF-07).
    let ctx =
        InspectionContext::from_lowercase_headers(page.status, &page.headers, config.ignore_waf);
    let verdict = WafInspector::inspect(&html, &ctx);
    if verdict.is_blocked {
        let chain = verdict.evidence_chain();
        log_scrape_error(
            &chain,
            url.as_str(),
            "fetch",
            Some(&correlation),
            "WAF challenge detected",
        );
        return Err(ScraperError::waf_blocked(url.to_string(), chain));
    }

    extract_content(&html, url, config, asset_downloader, engine, &correlation).await
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

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(miri))]
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
                || msg.contains("connect")
                || msg.contains("timed out")
                || msg.contains("timeout"),
            "expected connect-failure or timeout message, got: {msg}"
        );
    }

    // #583 acceptance tests: DOM discovery must honor max_depth.

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_max_depth_zero_returns_empty() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Seed page with 10 internal links — without the fix, all 10 would be
        // returned and crawled regardless of max_depth.
        let links: String = (1..=10)
            .map(|i| format!(r#"<a href="https://example.com/page/{i}">Link {i}</a>"#))
            .collect();
        let html = format!("<html><body><a href=\"/\">Home</a>{links}</body></html>");

        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&server)
            .await;

        let seed = Url::parse(&server.uri()).unwrap();
        let config = CrawlerConfig::builder(seed).max_depth(0).build();

        let urls = discover_urls_for_tui(&server.uri(), &config)
            .await
            .expect("discovery should succeed");

        assert!(
            urls.is_empty(),
            "max_depth=0 must return no discovered URLs, got {}",
            urls.len()
        );
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_max_depth_one_returns_links() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let seed_url = server.uri();
        // Links must share the seed's host to pass `is_internal_link`. The
        // external host must be filtered out.
        let html = format!(
            r#"<html><body>
            <a href="{seed_url}/page/1">Link 1</a>
            <a href="{seed_url}/page/2">Link 2</a>
            <a href="https://external.com/page">External</a>
        </body></html>"#
        );

        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&server)
            .await;

        let seed = Url::parse(&seed_url).unwrap();
        let config = CrawlerConfig::builder(seed).max_depth(1).build();

        let urls = discover_urls_for_tui(&seed_url, &config)
            .await
            .expect("discovery should succeed");

        // Internal links only (external.com filtered out by is_internal_link).
        assert_eq!(
            urls.len(),
            2,
            "max_depth=1 must return internal links from seed"
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

        let corr = CorrelationId::new();
        let result = scrape_single_url_for_tui(&dl, &url, &config, None, None, None, &corr)
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

        let corr = CorrelationId::new();
        let err = scrape_single_url_for_tui(&dl, &url, &config, None, None, None, &corr)
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

        let corr = CorrelationId::new();
        let result = scrape_single_url_for_tui(&dl, &url, &config, None, None, None, &corr)
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

        let corr = CorrelationId::new();
        let err = scrape_single_url_for_tui(&dl, &url, &config, None, None, None, &corr)
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

        let corr = CorrelationId::new();
        let err = scrape_single_url_for_tui(&dl, &url, &config, None, None, None, &corr)
            .await
            .expect_err("404 status should produce an error");

        assert!(
            matches!(err, ScraperError::Http { status: 404, .. }),
            "expected Http(404), got: {err:?}"
        );
    }

    // ─── extract_content direct tests (no Downloader needed) ───
    // All gated: extract_content → html_cleaner → lol_html → servo_arc
    // triggers Tree Borrows UB in servo_arc Arc::drop (upstream, experimental model).

    #[tokio::test]
    #[cfg(not(miri))]
    async fn extract_content_readability_extracts_article() {
        let html = r#"<html><head><title>Test Page</title></head><body>
            <article><h1>Hello</h1><p>This is a long enough paragraph of content that readability should be able to extract properly from the DOM structure.</p>
            <p>Second paragraph with more content to ensure readability has enough material to work with for extraction.</p></article>
        </body></html>"#;
        let url = Url::parse("https://example.com/article").unwrap();
        let config = ScraperConfig::default();

        let corr = CorrelationId::new();
        let result = extract_content(html, &url, &config, None, None, &corr).await;

        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(!content.content.is_empty());
        assert_eq!(content.url.as_str(), "https://example.com/article");
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn extract_content_fallback_short_content_returns_error() {
        let html = "<html><body><div></div></body></html>";
        let url = Url::parse("https://example.com/tiny").unwrap();
        let config = ScraperConfig::default();

        let corr = CorrelationId::new();
        let result = extract_content(html, &url, &config, None, None, &corr).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, ScraperError::ExtractionFailed { .. }),
            "Expected ExtractionFailed, got: {err:?}"
        );
    }

    #[tokio::test]
    #[cfg(not(miri))]
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

        let corr = CorrelationId::new();
        let result = extract_content(html, &url, &config, None, None, &corr).await;

        assert!(result.is_ok());
        let content = result.unwrap();
        assert!(content.content.contains("main content"));
    }

    #[tokio::test]
    #[cfg(not(miri))]
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

        let corr = CorrelationId::new();
        let result = extract_content(html, &url, &config, None, None, &corr).await;

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

    #[tokio::test]
    #[cfg(not(miri))]
    async fn extract_content_reuses_injected_correlation_id() {
        // Issue #501: callers own the page identity (e.g. the scrape span in
        // `scrape_single_url_for_tui`) and inject it so the exported content
        // is correlatable with the trace's `span_fields` — exact identity,
        // not a freshly generated one. The parameter is required: identity
        // enters through the type system or not at all.
        let html = r#"<html><head><title>Test Page</title></head><body>
            <article><h1>Hello</h1><p>This is a long enough paragraph of content that readability should be able to extract properly from the DOM structure.</p>
            <p>Second paragraph with more content to ensure readability has enough material to work with for extraction.</p></article>
        </body></html>"#;
        let url = Url::parse("https://example.com/article").unwrap();
        let config = ScraperConfig::default();
        let injected = CorrelationId::new_with_ids(uuid::Uuid::now_v7(), 0x00AB);

        let content = extract_content(html, &url, &config, None, None, &injected)
            .await
            .expect("extraction with an injected correlation ID must succeed");

        assert_eq!(
            content.correlation_id.as_ref(),
            Some(&injected),
            "extract_content must reuse the injected correlation ID exactly"
        );
    }
}
