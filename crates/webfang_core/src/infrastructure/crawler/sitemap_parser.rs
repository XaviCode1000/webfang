//! Sitemap Parser Module
//!
//! Zero-allocation streaming parser for XML sitemaps.
//! Supports gzip compression and sitemap index recursion.
//!
//! # Examples
//!
//! ```no_run
//! use webfang_core::infrastructure::crawler::SitemapParser;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let parser = SitemapParser::new()?;
//! let urls = parser.parse_from_url("https://example.com/sitemap.xml").await?;
//! println!("Found {} URLs", urls.len());
//! # Ok(())
//! # }
//! ```
//!
//! # Errors
//!
//! Returns `SitemapError` if:
//! - URL is invalid
//! - HTTP request fails
//! - XML parsing fails
//! - No `<loc>` elements found

use super::batch_processor::BatchProcessor;
use super::compression_handler::CompressionHandler;
use super::memory_manager::MemoryManager;
use super::retry_policy::RetryPolicy;
use super::sitemap_config::SitemapConfig;
use super::url_validator::UrlValidator;
use crate::domain::crawler_port::sitemap::SitemapParserPort;
use crate::domain::url_validation::is_internal_link;
use crate::domain::waf::InspectionContext;
use crate::domain::{CrawlError, UrlValidatorTrait};
use crate::infrastructure::http::waf_engine::WafInspector;
#[allow(unused_imports)]
use async_compression::tokio::bufread::GzipDecoder;
use futures::future::BoxFuture;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use url::Url;

// SitemapError, SitemapUrl and the sitemap `Result` alias moved to
// `domain::crawler_port::sitemap` in the sitemap port slice (ADR-0012-B,
// follow-up of #1082). Re-exported here so the `infrastructure::crawler`
// root paths keep resolving unchanged — the repo's blessed move+shim
// migration (ADR-0012 §6 tradeoff 2).
pub use crate::domain::crawler_port::sitemap::{Result, SitemapError, SitemapUrl};

/// Safely resolve a potentially-relative URL against a base URL.
///
/// Handles all RFC 3986 reference types: absolute, scheme-relative,
/// absolute-path, relative-path. Returns `None` for empty inputs.
///
/// Following **err-result-over-panic**: returns Option, never panics.
/// Following **perf-iter-over-index**: uses early returns, no loops.
///
/// # Arguments
///
/// * `base` - The base URL for resolution context
/// * `input` - The URL or path to resolve
///
/// # Returns
///
/// * `Some(Url)` - Successfully resolved URL
/// * `None` - Empty input or resolution failure
///
/// # Examples
///
/// ```
/// use url::Url;
/// use webfang_core::infrastructure::crawler::sitemap_parser::resolve_url;
///
/// let base = Url::parse("https://example.com/sitemap.xml").unwrap();
/// let resolved = resolve_url(&base, "/page.html").unwrap();
/// assert_eq!(resolved.as_str(), "https://example.com/page.html");
/// ```
#[must_use]
pub fn resolve_url(base: &Url, input: &str) -> Option<Url> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    // Fast path: already absolute - allow any absolute URL (explicit user intent)
    if input.starts_with("http://") || input.starts_with("https://") {
        return Url::parse(input).ok();
    }

    // RFC 3986 resolution for relative and protocol-relative URLs
    let resolved = base.join(input).ok()?;

    // Bug 3: post-resolution host validation (SSRF protection for protocol-relative/relative URLs)
    let seed_host = base.host_str().unwrap_or_default();
    if is_internal_link(resolved.as_str(), seed_host) {
        Some(resolved)
    } else {
        tracing::warn!(%resolved, "SSRF: protocol-relative URL escaped seed domain");
        None
    }
}

/// Zero-allocation streaming sitemap parser
///
/// Following mem-streaming-large-data: streaming parser, no buffer accumulation
pub struct SitemapParser {
    config: SitemapConfig,
    compression_handler: CompressionHandler,
    url_validator: UrlValidator,
    retry_policy: RetryPolicy,
    memory_manager: MemoryManager,
    batch_processor: BatchProcessor,
    /// TLS/HTTP2 fingerprint emulation preset applied to the sitemap fetch client.
    ///
    /// Threaded from the caller (ultimately the CLI `--h2-profile` value).
    /// Defaults to [`wreq_util::Profile::Chrome145`], preserving the historical
    /// sitemap fingerprint (#323).
    tls_emulation: wreq_util::Profile,
    /// Shared HTTP client used to fetch sitemap XML.
    ///
    /// Built once in the constructor from `tls_emulation` via the shared
    /// `create_http_client_with_config` factory (#299) and cloned cheaply
    /// (it is `Arc`-backed) into each retry attempt — no per-retry rebuild (#323).
    http_client: wreq::Client,
}

impl SitemapParser {
    /// Create new parser with default config
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`SitemapParser::with_config_and_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn new() -> std::result::Result<Self, CrawlError> {
        Self::with_config_and_profile(SitemapConfig::default(), wreq_util::Profile::Chrome145)
    }

    /// Create new parser with custom config
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`SitemapParser::with_config_and_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn with_config(config: SitemapConfig) -> std::result::Result<Self, CrawlError> {
        Self::with_config_and_profile(config, wreq_util::Profile::Chrome145)
    }

    /// Create new parser with custom config and an explicit TLS/H2 profile.
    ///
    /// The sitemap fetch client is built once here (via the shared
    /// `create_http_client_with_config` factory) from `tls_emulation` and reused
    /// across every fetch and retry, honoring the caller's `--h2-profile`
    /// selection instead of a hardcoded preset (#323).
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn with_config_and_profile(
        config: SitemapConfig,
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<Self, CrawlError> {
        let http_client = Self::build_client(tls_emulation)?;
        let max_decompressed_size = config.max_decompressed_size;
        Ok(Self {
            config,
            compression_handler: CompressionHandler::with_max_size(max_decompressed_size),
            url_validator: UrlValidator::with_profile(tls_emulation)?,
            retry_policy: RetryPolicy::new(),
            memory_manager: MemoryManager::new(),
            batch_processor: BatchProcessor::new(),
            tls_emulation,
            http_client,
        })
    }

    /// Build the sitemap fetch HTTP client for a given TLS/H2 profile.
    ///
    /// The request and connect timeouts are pinned to 10s to preserve the
    /// historical behavior of the previous hardcoded client (which set
    /// `.timeout(10s)`); only the TLS/H2 profile is now configurable (#323).
    /// The remaining settings (Chrome Client Hints, pool tuning, compression)
    /// come from the shared factory defaults (#299).
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the underlying client fails to build.
    fn build_client(
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<wreq::Client, CrawlError> {
        let http_config = crate::domain::http_config::HttpClientConfig {
            tls_emulation,
            timeout_secs: 10,
            connect_timeout_secs: 10,
            ..Default::default()
        };
        crate::infrastructure::http::create_http_client_with_config(&http_config)
            // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
            .map_err(|e| CrawlError::Internal(format!("failed to build sitemap client: {e}")))
    }

    /// Parse sitemap from URL (streaming, zero-allocation)
    ///
    /// # Arguments
    ///
    /// * `url` - Sitemap URL (supports .xml and .xml.gz)
    ///
    /// # Returns
    ///
    /// Vector of valid URLs found in sitemap
    ///
    /// # Errors
    ///
    /// Returns `SitemapError` if parsing fails or no URLs found
    ///
    /// Thin inherent wrapper over the [`SitemapParserPort`] impl (the logic
    /// moved there in the sitemap port slice, ADR-0012-B); infrastructure
    /// internals and integration-test call sites keep using this name.
    pub async fn parse_from_url(&self, url: &str) -> Result<Vec<SitemapUrl>> {
        SitemapParserPort::parse_from_url(self, url).await
    }

    /// Validate a sitemap HTTP response: status MUST be checked before
    /// content-type so a 404/5xx yields "not found" rather than the misleading
    /// "unexpected content-type" (issue #590, bug #9). Returns the content-type
    /// string on success for the caller's XML streaming path.
    fn validate_response(status: wreq::StatusCode, content_type: &str, url: &str) -> Result<()> {
        if !status.is_success() {
            tracing::warn!("Sitemap URL returned non-2xx status: {status} from {url}");
            return Err(SitemapError::HttpError {
                status: status.as_u16(),
                message: format!("server returned {status}"),
            });
        }
        let is_xml = content_type.is_empty()
            || content_type.contains("application/xml")
            || content_type.contains("text/xml")
            || content_type.contains("application/xhtml+xml")
            || url.ends_with(".xml")
            || url.ends_with(".xml.gz");
        if !is_xml {
            tracing::warn!(
                "Sitemap URL returned non-XML content type: {} from {url}",
                content_type
            );
            return Err(SitemapError::InvalidContentType(content_type.to_string()));
        }
        Ok(())
    }

    /// Internal recursive parser with depth tracking and loop detection
    async fn parse_with_depth(
        &self,
        url: &str,
        depth: u8,
        visited: &Arc<Mutex<HashSet<Url>>>,
    ) -> Result<Vec<SitemapUrl>> {
        // Base case: max depth reached
        if depth == 0 {
            return Err(SitemapError::MaxDepthExceeded);
        }

        let base_url = Url::parse(url)?;

        // Loop detection: skip already-visited URLs
        {
            let visited_lock = visited.lock().map_err(|e| SitemapError::HttpError {
                status: 0,
                message: format!("failed to acquire visited lock: {e}"),
            })?;
            if visited_lock.contains(&base_url) {
                tracing::warn!("Skipping already-visited sitemap (loop detected): {}", url);
                return Err(SitemapError::InvalidStructure);
            }
        }

        // [3.6] RetryPolicy: wrap HTTP request with retry logic.
        // The client is built once in the constructor (honoring tls_emulation)
        // and cloned cheaply (Arc-backed) into each attempt — no per-retry rebuild (#323).
        let response = self
            .retry_policy
            .execute_with_retry(|| {
                let url = url.to_string();
                let client = self.http_client.clone();
                async move { client.get(&url).send().await }
            })
            .await
            .map_err(|e| SitemapError::HttpError {
                status: 0, // Unknown status for network/connection errors
                message: e.to_string(),
            })?;

        // Validate response: status checked before content-type (issue #590, bug #9).
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Self::validate_response(status, content_type.as_str(), url)?;

        // Mark URL as visited after successful fetch
        {
            let mut visited_lock = visited.lock().map_err(|e| SitemapError::HttpError {
                status: 0,
                message: format!("failed to acquire visited lock: {e}"),
            })?;
            visited_lock.insert(base_url.clone());
        }

        // Stream response with size limit
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut raw_bytes = Vec::with_capacity(8192);
        let mut total_bytes = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| SitemapError::HttpError {
                status: 0, // Stream errors are connection errors
                message: e.to_string(),
            })?;
            total_bytes += chunk.len();
            if total_bytes > self.config.max_response_size {
                tracing::warn!(
                    "Sitemap response too large: {} bytes from {}",
                    total_bytes,
                    url
                );
                return Err(SitemapError::ResponseTooLarge(
                    self.config.max_response_size,
                ));
            }
            raw_bytes.extend_from_slice(&chunk);
        }

        // [3.4] CompressionHandler integration: detect and decompress content
        let decompressed = self
            .compression_handler
            .detect_and_decompress(&raw_bytes, url)
            .await
            .map_err(|e| SitemapError::DecompressionError(e.to_string()))?;

        // Issue #879 (Option A): inspect the body BEFORE XML parsing so a WAF
        // challenge served where sitemap content should be surfaces as the
        // typed error instead of generic XML-garbage failures.
        if let Some(err) =
            Self::waf_challenge_error(&decompressed, url, status.as_u16(), &content_type)
        {
            return Err(err);
        }

        // Parse using unified decompression handle
        let (urls, is_index) = if decompressed.is_empty() {
            return Err(SitemapError::NoUrlsFound);
        } else {
            self.parse_xml_sitemap(&decompressed, &base_url).await?
        };

        // [3.7] MemoryManager: handle disk swapping for large result sets
        // (both branches need it, so hoist it above the index/urlset split).
        self.memory_manager
            .handle_disk_swapping(&urls)
            .map_err(|e| SitemapError::HttpError {
                status: 0,
                message: format!("memory management failed: {e}"),
            })?;

        // Check if sitemap index (recursive) - Bug 7: use root element detection
        if is_index {
            tracing::debug!(
                "Detected sitemap index (root element), recursing (depth: {})",
                depth
            );

            self.parse_sitemap_index(&urls, depth - 1, visited).await
        } else {
            // [3.8] BatchProcessor: apply crawl budget optimization
            let optimized_urls = self.batch_processor.apply_crawl_budget(urls, &self.config);

            Ok(optimized_urls)
        }
    }

    /// Parse gzip-compressed sitemap
    #[allow(dead_code)]
    async fn parse_gzip_sitemap(&self, bytes: &[u8], base_url: &Url) -> Result<Vec<SitemapUrl>> {
        use tokio::io::{AsyncReadExt, BufReader};
        let reader = BufReader::new(bytes);
        let mut decoder = GzipDecoder::new(reader);

        let mut limited =
            AsyncReadExt::take(&mut decoder, self.config.max_decompressed_size as u64);
        let mut decompressed = Vec::new();
        AsyncReadExt::read_to_end(&mut limited, &mut decompressed).await?;

        if decompressed.len() >= self.config.max_decompressed_size {
            tracing::warn!(
                "Gzip decompression hit size limit ({} bytes) — possible decompression bomb",
                decompressed.len()
            );
            return Err(SitemapError::DecompressedTooLarge(
                self.config.max_decompressed_size,
            ));
        }

        let (urls, _is_index) = self.parse_xml_sitemap(&decompressed, base_url).await?;
        Ok(urls)
    }

    /// Parse XML sitemap (zero-allocation streaming) with metadata extraction
    ///
    /// Extracts `<loc>`, `<lastmod>`, `<priority>`, and `<changefreq>` from
    /// `<url>` entries per sitemaps.org spec. Also handles `<sitemap>` entries
    /// in sitemap index files.
    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    async fn parse_xml_sitemap(
        &self,
        bytes: &[u8],
        base_url: &Url,
    ) -> Result<(Vec<SitemapUrl>, bool)> {
        let mut reader = Reader::from_reader(bytes);
        let (mut urls, is_index) =
            parse_sitemap_core(&mut reader, base_url, true, Some(&self.url_validator))?;

        if urls.is_empty() {
            return Err(SitemapError::NoUrlsFound);
        }
        dedup_and_sort_sitemap_urls(&mut urls);
        Ok((urls, is_index))
    }

    /// Inspect a fetched sitemap-chain body for WAF/CAPTCHA challenges
    /// (issue #879).
    ///
    /// Reuses [`WafInspector`] with full HTTP context (status/content-type are
    /// known here), so verdict semantics match the HTTP client path exactly:
    /// Challenge-tier markers block at any status, Fingerprint-tier evidence
    /// blocks only when correlated with a WAF status code (REQ-WAF-05/09).
    /// Control headers are not re-collected here because `validate_response`
    /// guarantees a 2xx status, under which Fingerprint evidence never blocks.
    fn waf_challenge_error(
        body: &[u8],
        url: &str,
        status: u16,
        content_type: &str,
    ) -> Option<SitemapError> {
        let ctx = InspectionContext {
            status: Some(status),
            content_type: (!content_type.is_empty()).then(|| content_type.to_string()),
            headers: std::collections::HashMap::new(),
            ignore_waf: false,
        };
        let text = String::from_utf8_lossy(body);
        let verdict = WafInspector::inspect(&text, &ctx);
        if !verdict.is_blocked {
            return None;
        }
        tracing::warn!(
            url = %url,
            status = %status,
            evidences = verdict.evidences.len(),
            "WAF/CAPTCHA challenge detected in sitemap body; aborting"
        );
        Some(SitemapError::WafChallenge {
            url: url.to_string(),
            provider: verdict.evidence_chain(),
        })
    }

    /// Parse sitemap index recursively with error propagation
    async fn parse_sitemap_index(
        &self,
        sitemap_urls: &[SitemapUrl],
        depth: u8,
        visited: &Arc<Mutex<HashSet<Url>>>,
    ) -> Result<Vec<SitemapUrl>> {
        use futures::stream::{self, StreamExt};

        let mut all_urls = Vec::new();
        let mut failures = Vec::new();

        let results = stream::iter(sitemap_urls.iter().cloned())
            .map(|sitemap_url| {
                let visited = visited.clone();
                async move {
                    let url = sitemap_url.url.clone();
                    let result = self.parse_with_depth(url.as_str(), depth, &visited).await;
                    (url, result)
                }
            })
            .buffered(self.config.concurrency)
            .collect::<Vec<_>>()
            .await;

        for (url, result) in results {
            match result {
                Ok(urls) => all_urls.extend(urls),
                Err(e) => {
                    tracing::warn!("Failed to parse sitemap {}: {}", url, e);
                    failures.push((url, e));
                },
            }
        }

        // Issue #879 (Option A): a challenge on ANY child hard-aborts the
        // whole index (HybridRouter parity) — the host is serving challenges,
        // not sitemaps — instead of being swallowed into AllChildrenFailed.
        for (_, e) in &failures {
            if let SitemapError::WafChallenge { url, provider } = e {
                tracing::warn!(
                    url = %url,
                    provider = %provider,
                    "WAF/CAPTCHA challenge detected in index child; aborting index"
                );
                return Err(SitemapError::WafChallenge {
                    url: url.clone(),
                    provider: provider.clone(),
                });
            }
        }

        if all_urls.is_empty() {
            if !failures.is_empty() {
                // Split children that were valid but EMPTY (NoUrlsFound) from
                // children that genuinely failed to fetch/parse. An index whose
                // children are all empty is "no URLs discovered" (exit 2 via
                // NoUrlsFound→SitemapEmpty), NOT an infrastructure failure; only
                // real fetch/parse failures yield AllChildrenFailed (exit 69 via
                // Parse→Internal). Previously both collapsed into
                // AllChildrenFailed and the CLI string-matched the message to
                // tell them apart (stabilization-sitemap-regression).
                let (empty_children, real_failures): (Vec<_>, Vec<_>) = failures
                    .into_iter()
                    .partition(|(_, e)| matches!(e, SitemapError::NoUrlsFound));
                if real_failures.is_empty() {
                    tracing::debug!(
                        "all {} child sitemaps were empty — no URLs discovered",
                        empty_children.len()
                    );
                    return Err(SitemapError::NoUrlsFound);
                }
                let failure_msgs: Vec<String> = real_failures
                    .iter()
                    .map(|(url, e)| format!("{url}: {e}"))
                    .collect();
                return Err(SitemapError::AllChildrenFailed(
                    real_failures.len(),
                    failure_msgs.join("; "),
                ));
            }
            Err(SitemapError::NoUrlsFound)
        } else {
            dedup_and_sort_sitemap_urls(&mut all_urls);
            Ok(all_urls)
        }
    }

    /// Check if gzip is enabled in config
    #[must_use]
    pub fn has_gzip(&self) -> bool {
        self.config.gzip_enabled
    }

    /// Get current max depth
    #[must_use]
    pub fn max_depth(&self) -> u8 {
        self.config.max_depth
    }

    /// Get the TLS/HTTP2 fingerprint emulation preset used for sitemap fetches.
    #[must_use]
    pub fn tls_emulation(&self) -> wreq_util::Profile {
        self.tls_emulation
    }
}

/// Domain-port implementation (ADR-0012-B sitemap port). The `parse_from_url`
/// logic moved here from the inherent method; the inherent `parse_from_url`
/// is now a thin wrapper so infrastructure internals and integration-test
/// call sites keep compiling unchanged.
impl SitemapParserPort for SitemapParser {
    fn parse_from_url<'a>(
        &'a self,
        sitemap_url: &'a str,
    ) -> BoxFuture<'a, Result<Vec<SitemapUrl>>> {
        Box::pin(async move {
            let visited = Arc::new(Mutex::new(HashSet::new()));
            self.parse_with_depth(sitemap_url, self.config.max_depth, &visited)
                .await
        })
    }
}

/// Shared core streaming parser for sitemap XML.
///
/// Extracts `<loc>`, `<lastmod>`, `<priority>`, and `<changefreq>` from `<url>`
/// entries. When `handle_sitemap_index` is true, also handles `<sitemap>`
/// entries in sitemap index files and returns `is_index`.
///
/// Uses `read_text_into` for robust `<loc>` content consumption (handles
/// mixed Text/CDATA/GeneralRef). When `url_validator` is provided, filters
/// invalid URL patterns per domain rules.
#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
fn parse_sitemap_core<R>(
    reader: &mut Reader<R>,
    base_url: &Url,
    handle_sitemap_index: bool,
    url_validator: Option<&UrlValidator>,
) -> Result<(Vec<SitemapUrl>, bool)>
where
    R: std::io::BufRead,
{
    let mut urls = Vec::new();
    let mut buf = Vec::new();
    let mut root_tag: Option<Vec<u8>> = None;
    let mut is_index = false;
    let mut saw_element = false;

    // State machine for metadata parsing
    let mut in_url = false;
    let mut in_sitemap = false;
    let mut in_loc = false;
    let mut in_lastmod = false;
    let mut in_priority = false;
    let mut in_changefreq = false;
    let mut current_url: Option<Url> = None;
    let mut current_lastmod: Option<String> = None;
    let mut current_priority: Option<f32> = None;
    let mut current_changefreq: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                saw_element = true;
                let name = e.name();

                // Bug 7: classify by root element (only when handling index)
                if handle_sitemap_index && root_tag.is_none() {
                    root_tag = match name.as_ref() {
                        b"sitemapindex" => {
                            is_index = true;
                            Some(name.as_ref().to_vec())
                        },
                        b"urlset" => Some(name.as_ref().to_vec()),
                        _ => return Err(SitemapError::InvalidStructure),
                    };
                }

                if name.as_ref() == b"url" {
                    in_url = true;
                    current_url = None;
                    current_lastmod = None;
                    current_priority = None;
                    current_changefreq = None;
                } else if handle_sitemap_index && name.as_ref() == b"sitemap" {
                    in_sitemap = true;
                    current_url = None;
                    current_lastmod = None;
                } else if (in_url || (handle_sitemap_index && in_sitemap))
                    && name.as_ref() == b"loc"
                {
                    in_loc = true;
                } else if in_url && name.as_ref() == b"lastmod" {
                    in_lastmod = true;
                } else if in_url && name.as_ref() == b"priority" {
                    in_priority = true;
                } else if in_url && name.as_ref() == b"changefreq" {
                    in_changefreq = true;
                }
            },
            Ok(Event::End(ref e)) => {
                let name = e.name();
                if name.as_ref() == b"url" {
                    in_url = false;
                    if let Some(ref url) = current_url {
                        urls.push(SitemapUrl {
                            url: url.clone(),
                            lastmod: current_lastmod.clone(),
                            priority: current_priority,
                            changefreq: current_changefreq.clone(),
                        });
                    }
                } else if handle_sitemap_index && name.as_ref() == b"sitemap" {
                    in_sitemap = false;
                    if let Some(ref url) = current_url {
                        urls.push(SitemapUrl {
                            url: url.clone(),
                            lastmod: current_lastmod.clone(),
                            priority: None,
                            changefreq: None,
                        });
                    }
                } else if name.as_ref() == b"loc" {
                    in_loc = false;
                } else if name.as_ref() == b"lastmod" {
                    in_lastmod = false;
                } else if name.as_ref() == b"priority" {
                    in_priority = false;
                } else if name.as_ref() == b"changefreq" {
                    in_changefreq = false;
                }
            },
            Ok(Event::Text(ref e)) if in_loc => {
                // Fallback for text content not captured by read_text_into
                let text = e
                    .decode()
                    .map_err(|e| SitemapError::XmlError(e.to_string()))?;
                let url_str = text.trim();
                if !url_str.is_empty() {
                    if let Some(url) = resolve_url(base_url, url_str) {
                        apply_url_validation(url, url_validator, &mut current_url);
                    }
                }
            },
            Ok(Event::CData(ref e)) if in_loc => {
                // Handle CDATA content in <loc> (e.g., <loc><![CDATA[https://example.com]]></loc>)
                let url_str = String::from_utf8_lossy(e).trim().to_string();
                if !url_str.is_empty() {
                    if let Some(url) = resolve_url(base_url, &url_str) {
                        apply_url_validation(url, url_validator, &mut current_url);
                    }
                }
            },
            Ok(Event::Text(ref e)) if in_lastmod => {
                if let Ok(text) = e.decode() {
                    current_lastmod = Some(text.trim().to_string());
                }
            },
            Ok(Event::Text(ref e)) if in_priority => {
                if let Ok(text) = e.decode() {
                    if let Ok(p) = text.trim().parse::<f32>() {
                        current_priority = Some(p.clamp(0.0, 1.0));
                    }
                }
            },
            Ok(Event::Text(ref e)) if in_changefreq => {
                if let Ok(text) = e.decode() {
                    current_changefreq = Some(text.trim().to_string());
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(SitemapError::XmlError(e.to_string())),
            _ => {},
        }
        buf.clear();
    }

    // A body with no XML elements at all (e.g. plain text garbage) is NOT an
    // empty sitemap — it has no recognizable structure. Distinguishing this
    // from a valid-but-empty `<urlset/>` keeps AllChildrenFailed counts honest
    // (stabilization-sitemap-regression).
    if !saw_element {
        return Err(SitemapError::InvalidStructure);
    }

    Ok((urls, is_index))
}

/// Deduplicate and sort SitemapUrl entries for deterministic ordering.
///
/// 1. Sorts by URL and deduplicates (preserves first occurrence with metadata)
/// 2. Sorts by priority descending, then lastmod descending
fn dedup_and_sort_sitemap_urls(urls: &mut Vec<SitemapUrl>) {
    // Deduplicate by URL (preserve first occurrence with metadata)
    urls.sort_by(|a, b| a.url.cmp(&b.url));
    urls.dedup_by(|a, b| a.url == b.url);

    // Sort for deterministic ordering: priority desc, then lastmod desc
    urls.sort_by(|a, b| {
        let pri_a = a.priority.unwrap_or(0.5);
        let pri_b = b.priority.unwrap_or(0.5);
        pri_b
            .partial_cmp(&pri_a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.lastmod.cmp(&a.lastmod))
    });
}

/// Apply URL validation and update current_url if valid.
fn apply_url_validation(
    url: Url,
    url_validator: Option<&UrlValidator>,
    current_url: &mut Option<Url>,
) {
    if let Some(validator) = url_validator {
        let validation = validator.filter_invalid_patterns(&url);
        match validation {
            crate::domain::ValidationResult::Valid => {
                *current_url = Some(url);
            },
            crate::domain::ValidationResult::Invalid(reason) => {
                tracing::debug!("Filtered invalid URL: {} — {}", url, reason);
            },
            crate::domain::ValidationResult::NeedsRedirect(new_url) => {
                *current_url = Some(new_url);
            },
        }
    } else {
        *current_url = Some(url);
    }
}

/// Parse sitemap XML content using quick-xml (streaming parser)
///
/// Standalone streaming parser for sitemap entries with metadata,
/// extracted from the application layer (issue #442): XML parsing is an
/// infrastructure concern and lives here alongside [`SitemapParser`].
/// Relative URLs are resolved against `base_url`.
///
/// Following **xml-no-regex**: Uses quick-xml instead of regex for XML parsing.
/// Following **mem-stream-processing**: Streaming approach avoids loading entire DOM.
///
/// # Arguments
///
/// * `xml_content` - XML content of the sitemap
/// * `base_url` - Base URL to resolve relative entries against
///
/// # Returns
///
/// * `Ok(Vec<SitemapUrl>)` - List of URLs with metadata (empty if no URLs found)
/// * `Err(CrawlError)` - Parse error
#[allow(clippy::too_many_lines)]
pub fn parse_sitemap(
    xml_content: &str,
    base_url: &Url,
) -> std::result::Result<Vec<SitemapUrl>, CrawlError> {
    let mut reader = Reader::from_str(xml_content);
    let (mut urls, _is_index) = parse_sitemap_core(&mut reader, base_url, false, None)
        .map_err(|e| CrawlError::Parse(e.to_string()))?;

    // Lenient: return empty vec for empty sitemaps (matches old behavior for tests/benchmarks)
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    dedup_and_sort_sitemap_urls(&mut urls);

    Ok(urls)
}

#[cfg(all(test, not(miri)))]
mod waf_inspection_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ── Issue #879 (Option A): WAF inspection over sitemap bodies ──

    /// Cloudflare Turnstile widget marker — Challenge-tier (T1), blocks even
    /// in degraded mode per REQ-WAF-05/09.
    const CHALLENGE_HTML: &str = r#"<html><body>Just a moment...</body><div id="cf-turnstile" data-sitekey="abc"></div></html>"#;

    /// Shared writer capturing tracing fmt output for assertions.
    #[derive(Clone)]
    struct SharedWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedWriter {
        type Writer = SharedWriterGuard;

        fn make_writer(&'a self) -> Self::Writer {
            SharedWriterGuard(self.0.clone())
        }
    }

    struct SharedWriterGuard(Arc<std::sync::Mutex<Vec<u8>>>);

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

    /// A WAF challenge page served where sitemap XML should be must surface the
    /// typed `SitemapError::WafChallenge` with host context and evidence chain,
    /// plus a structured trace event — not a generic XML parse failure (#879).
    #[test]
    fn parse_from_url_waf_challenge_body_returns_typed_error_and_trace_event() {
        let buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
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
                    .and(path("/sitemap.xml"))
                    .respond_with(
                        ResponseTemplate::new(200)
                            .insert_header("content-type", "text/html")
                            .set_body_string(CHALLENGE_HTML),
                    )
                    .mount(&mock)
                    .await;

                let parser = SitemapParser::new().unwrap();
                let result = parser
                    .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
                    .await;

                match result {
                    Err(SitemapError::WafChallenge { url, provider }) => {
                        assert_eq!(url, format!("{}/sitemap.xml", mock.uri()));
                        assert!(
                            provider.contains("Cloudflare"),
                            "evidence chain expected in provider, got: {provider}"
                        );
                    },
                    other => panic!("expected SitemapError::WafChallenge, got: {other:?}"),
                }
            });
        });

        let captured = buf.lock().expect("trace buffer lock").clone();
        let out = String::from_utf8_lossy(&captured);
        assert!(
            out.contains("challenge"),
            "WAF trace event expected on the parser path, got: {out}"
        );
    }

    /// A challenge served by ONE child of a sitemap index must hard-abort the
    /// whole index with `WafChallenge` (HybridRouter parity) instead of being
    /// swallowed into `AllChildrenFailed` (#879).
    #[tokio::test]
    async fn index_child_waf_challenge_propagates_typed_error() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                 <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\
                 <sitemap><loc>{}/child.xml</loc></sitemap>\
                 </sitemapindex>",
                mock.uri()
            )))
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/child.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string(CHALLENGE_HTML),
            )
            .mount(&mock)
            .await;

        let parser = SitemapParser::new().unwrap();
        let result = parser
            .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
            .await;

        match result {
            Err(SitemapError::WafChallenge { url, .. }) => {
                assert_eq!(url, format!("{}/child.xml", mock.uri()));
            },
            other => panic!("expected SitemapError::WafChallenge from index child, got: {other:?}"),
        }
    }

    /// A benign sitemap body that merely mentions a WAF vendor at status 200 is
    /// Fingerprint-tier evidence and must NOT raise `WafChallenge` (REQ-WAF-09).
    #[tokio::test]
    async fn benign_sitemap_body_mentioning_vendor_is_not_a_challenge() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <!-- served behind cloudflare -->
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/page1</loc></url>
        </urlset>"#;
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sitemap.xml"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/xml")
                    .set_body_string(xml),
            )
            .mount(&mock)
            .await;

        let parser = SitemapParser::new().unwrap();
        let result = parser
            .parse_from_url(&format!("{}/sitemap.xml", mock.uri()))
            .await;
        assert!(
            !matches!(result, Err(SitemapError::WafChallenge { .. })),
            "benign vendor mention must not raise WafChallenge, got: {result:?}"
        );
        assert!(
            result.is_ok(),
            "valid XML must still parse, got: {result:?}"
        );
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_parse_simple_sitemap() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/page1</loc></url>
            <url><loc>https://example.com/page2</loc></url>
            <url><loc>https://example.com/page3</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new().unwrap();
        let base = Url::parse("https://example.com").unwrap();
        let (urls, _is_index) = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert_eq!(urls.len(), 3);
        assert!(urls
            .iter()
            .any(|u| u.url.as_str() == "https://example.com/page1"));
    }

    #[tokio::test]
    async fn test_parse_sitemap_with_duplicates() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/page1</loc></url>
            <url><loc>https://example.com/page1</loc></url>
            <url><loc>https://example.com/page2</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new().unwrap();
        let base = Url::parse("https://example.com").unwrap();
        let (urls, _is_index) = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert_eq!(urls.len(), 2);
    }

    #[tokio::test]
    async fn test_parse_empty_sitemap() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        </urlset>"#;

        let parser = SitemapParser::new().unwrap();
        let base = Url::parse("https://example.com").unwrap();
        let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

        assert!(matches!(result, Err(SitemapError::NoUrlsFound)));
    }

    #[tokio::test]
    async fn test_parse_malformed_xml() {
        let xml = r#"<?xml version="1.0"?>
        <urlset>
            <url><loc>https://example.com/page1</loc>
            <!-- Missing closing tag -->
        </urlset>"#;

        let parser = SitemapParser::new().unwrap();
        let base = Url::parse("https://example.com").unwrap();
        let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

        // Malformed XML should either parse (lenient parser) or fail with XmlError
        match &result {
            Ok(_) => {}, // Parser is lenient, this is acceptable
            Err(e) => assert!(
                matches!(e, SitemapError::XmlError(_)),
                "expected XmlError for malformed XML, got: {e:?}"
            ),
        }
    }

    #[test]
    fn test_config_builder() {
        let config = SitemapConfig::builder()
            .gzip_enabled(true)
            .max_depth(5)
            .concurrency(10)
            .build();

        assert!(config.gzip_enabled);
        assert_eq!(config.max_depth, 5);
        assert_eq!(config.concurrency, 10);
    }

    #[test]
    fn test_config_default() {
        let config = SitemapConfig::default();

        assert!(config.gzip_enabled);
        assert_eq!(config.max_depth, 3);
        assert_eq!(config.concurrency, 5);
    }

    #[test]
    fn test_parser_has_gzip() {
        let parser_gzip =
            SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(true).build())
                .unwrap();
        assert!(parser_gzip.has_gzip());

        let parser_no_gzip =
            SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(false).build())
                .unwrap();
        assert!(!parser_no_gzip.has_gzip());
    }

    #[tokio::test]
    async fn test_filter_invalid_schemes() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/valid</loc></url>
            <url><loc>http://example.com/valid</loc></url>
            <url><loc>ftp://example.com/invalid</loc></url>
            <url><loc>file:///etc/passwd</loc></url>
            <url><loc>javascript:alert(1)</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new().unwrap();
        let base = Url::parse("https://example.com").unwrap();
        let (urls, _is_index) = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert_eq!(urls.len(), 2);
        assert!(urls
            .iter()
            .all(|u| u.url.scheme() == "http" || u.url.scheme() == "https"));
    }

    /// Bug #9 regression: a 404 response must yield SitemapError::HttpError
    /// ("server returned 404"), NOT SitemapError::InvalidContentType. Status
    /// MUST be checked BEFORE content-type (issue #590).
    #[tokio::test]
    async fn test_sitemap_404_yields_http_error_not_content_type() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let port = server.address().port();

        Mock::given(path("/sitemap.xml"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
            .mount(&server)
            .await;

        let parser = SitemapParser::new().unwrap();
        let result = parser
            .parse_from_url(&format!("http://127.0.0.1:{port}/sitemap.xml"))
            .await;

        match result {
            Err(SitemapError::HttpError { message, .. }) => {
                assert!(
                    message.contains("404"),
                    "error must reference 404, got: {message}"
                );
            },
            other => panic!("expected SitemapError::HttpError with 404, got: {other:?}"),
        }
    }

    /// Compress `data` with gzip using the existing workspace dependency
    /// (`async-compression` only exposes `tokio::bufread` adapters).
    async fn gzip_compress(data: &[u8]) -> Vec<u8> {
        use async_compression::tokio::bufread::GzipEncoder;
        use tokio::io::{AsyncReadExt, BufReader};

        let mut encoder = GzipEncoder::new(BufReader::new(std::io::Cursor::new(data)));
        let mut out = Vec::new();
        encoder.read_to_end(&mut out).await.unwrap();
        out
    }

    /// #757 regression — the MDN case: a server serves an `.xml.gz` sitemap
    /// whose body is gzip AND sets `content-encoding: gzip` for transport.
    /// The HTTP client (`wreq` built with `.gzip(true)`) auto-decompresses the
    /// transport layer and strips `Content-Encoding`, delivering plain XML.
    /// Before the fix, `CompressionHandler` trusted the `.gz` extension and
    /// decompressed again -> "decompression failed: Invalid gzip header".
    /// After the fix, extension without magic bytes passes through.
    #[tokio::test]
    async fn test_sitemap_gz_with_content_encoding_header() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let port = server.address().port();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://developer.mozilla.org/en-US/docs/page1</loc></url>
            <url><loc>https://developer.mozilla.org/en-US/docs/page2</loc></url>
        </urlset>"#;

        Mock::given(path("/sitemap.xml.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(gzip_compress(xml.as_bytes()).await, "application/gzip")
                    .insert_header("content-encoding", "gzip"),
            )
            .mount(&server)
            .await;

        let parser = SitemapParser::new().unwrap();
        let urls = parser
            .parse_from_url(&format!("http://127.0.0.1:{port}/sitemap.xml.gz"))
            .await
            .expect("body gzip + content-encoding gzip must parse (#757)");
        assert_eq!(urls.len(), 2);
    }

    /// #757 case (b): a gzip body behind a `.gz` URL WITHOUT a transport
    /// `content-encoding` header. Today this works; it must keep working —
    /// magic-byte sniffing detects gzip and the handler decompresses.
    #[tokio::test]
    async fn test_sitemap_gz_body_without_content_encoding() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let port = server.address().port();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/page1</loc></url>
        </urlset>"#;

        Mock::given(path("/manual.xml.gz"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(gzip_compress(xml.as_bytes()).await, "application/gzip"),
            )
            .mount(&server)
            .await;

        let parser = SitemapParser::new().unwrap();
        let urls = parser
            .parse_from_url(&format!("http://127.0.0.1:{port}/manual.xml.gz"))
            .await
            .expect("gzip body without content-encoding must still decompress");
        assert_eq!(urls.len(), 1);
    }

    /// #757 case (c): a plain (already-decoded) body behind a lying `.gz` URL
    /// with no compression headers. Before the fix this failed with
    /// "Invalid gzip header" because the extension forced decompression;
    /// after the fix, magic-byte sniffing passes it through untouched.
    #[tokio::test]
    async fn test_sitemap_lying_gz_extension_with_plain_body() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let port = server.address().port();

        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
            <url><loc>https://example.com/page1</loc></url>
        </urlset>"#;

        Mock::given(path("/sitemap.xml.gz"))
            .respond_with(ResponseTemplate::new(200).set_body_string(xml))
            .mount(&server)
            .await;

        let parser = SitemapParser::new().unwrap();
        let urls = parser
            .parse_from_url(&format!("http://127.0.0.1:{port}/sitemap.xml.gz"))
            .await
            .expect("plain body behind .gz URL must pass through (#757)");
        assert_eq!(urls.len(), 1);
    }

    #[tokio::test]
    async fn test_parse_sitemap_with_namespaces() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
                xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
            <url>
                <loc>https://example.com/page1</loc>
                <image:image><image:loc>https://example.com/image.jpg</image:loc></image:image>
            </url>
            <url><loc>https://example.com/page2</loc></url>
        </urlset>"#;

        let parser = SitemapParser::new().unwrap();
        let base = Url::parse("https://example.com").unwrap();
        let (urls, _is_index) = parser
            .parse_xml_sitemap(xml.as_bytes(), &base)
            .await
            .unwrap();

        assert!(urls.len() >= 2);
    }

    #[test]
    fn test_parse_sitemap_max_depth_exceeded() {
        // The `SitemapError::MaxDepthExceeded` Display assertion moved with the
        // error type to `domain::crawler_port::sitemap::tests` (ADR-0012-B);
        // the config-side invariant (depth 0 is representable) stays here.
        let config = SitemapConfig::builder().max_depth(0).build();
        assert_eq!(config.max_depth, 0);
    }

    #[test]
    fn test_resolve_url_relative_paths() {
        let base = Url::parse("https://example.com/sitemap.xml").unwrap();

        let resolved = resolve_url(&base, "../page").unwrap();
        assert_eq!(resolved.as_str(), "https://example.com/page");

        let resolved = resolve_url(&base, "page.html").unwrap();
        assert_eq!(resolved.as_str(), "https://example.com/page.html");

        let resolved = resolve_url(&base, "/page").unwrap();
        assert_eq!(resolved.as_str(), "https://example.com/page");

        // Protocol-relative URL to different host is blocked by SSRF protection (Bug 3)
        assert!(resolve_url(&base, "//other/page").is_none());
    }

    #[test]
    fn test_resolve_url_empty_input() {
        let base = Url::parse("https://example.com").unwrap();

        assert!(resolve_url(&base, "").is_none());
        assert!(resolve_url(&base, "   ").is_none());
    }

    #[test]
    fn test_config_builder_zero_falls_back_to_defaults() {
        let config = SitemapConfig::builder()
            .max_response_size(0)
            .max_decompressed_size(0)
            .build();

        assert_eq!(config.max_response_size, 52_428_800);
        assert_eq!(config.max_decompressed_size, 104_857_600);
    }

    // -- Mutation-killing tests for sitemap_parser --

    // Gap A: resolve_url — absolute URL passthrough
    #[test]
    fn test_resolve_url_absolute_passthrough() {
        let base = Url::parse("https://example.com/sitemap.xml").unwrap();

        let resolved = resolve_url(&base, "https://other.com/page").unwrap();
        assert_eq!(resolved.as_str(), "https://other.com/page");

        let resolved = resolve_url(&base, "http://insecure.com/page").unwrap();
        assert_eq!(resolved.as_str(), "http://insecure.com/page");
    }

    #[test]
    fn test_resolve_url_absolute_overrides_base() {
        let base = Url::parse("https://example.com/sitemap.xml").unwrap();
        let resolved = resolve_url(&base, "https://completely-different.org/path").unwrap();
        assert_eq!(resolved.host_str(), Some("completely-different.org"));
    }

    // Gap B: parse_with_depth — depth=0 returns MaxDepthExceeded without HTTP
    #[tokio::test]
    async fn test_parse_from_url_depth_zero_returns_error() {
        let config = SitemapConfig::builder().max_depth(0).build();
        let parser = SitemapParser::with_config(config).unwrap();
        let result = parser
            .parse_from_url("https://example.com/sitemap.xml")
            .await;
        assert!(matches!(result, Err(SitemapError::MaxDepthExceeded)));
    }

    #[tokio::test]
    #[ignore = "requires network — hits real DNS for invalid-host-xyz-12345.com"]
    async fn test_parse_from_url_depth_one_attempts_fetch() {
        let config = SitemapConfig::builder().max_depth(1).build();
        let parser = SitemapParser::with_config(config).unwrap();
        // depth=1 means it tries the HTTP fetch — with an invalid host it should fail
        let result = parser
            .parse_from_url("https://invalid-host-xyz-12345.com/sitemap.xml")
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_max_depth_accessor() {
        let parser =
            SitemapParser::with_config(SitemapConfig::builder().max_depth(7).build()).unwrap();
        assert_eq!(parser.max_depth(), 7);
    }

    #[test]
    fn test_max_depth_default() {
        let parser = SitemapParser::new().unwrap();
        assert_eq!(parser.max_depth(), 3);
    }

    // -- #323: tls_emulation is honored, not hardcoded --

    #[test]
    fn test_new_defaults_to_chrome145_profile() {
        let parser = SitemapParser::new().unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome145);
    }

    #[test]
    fn test_with_config_defaults_to_chrome145_profile() {
        let parser = SitemapParser::with_config(SitemapConfig::default()).unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome145);
    }

    #[test]
    fn test_with_config_and_profile_accepts_custom_profile() {
        let parser = SitemapParser::with_config_and_profile(
            SitemapConfig::default(),
            wreq_util::Profile::Chrome131,
        )
        .unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome131);
    }

    #[test]
    fn test_with_config_and_profile_accepts_firefox_profile() {
        let parser = SitemapParser::with_config_and_profile(
            SitemapConfig::builder().max_depth(2).build(),
            wreq_util::Profile::Firefox135,
        )
        .unwrap();
        assert_eq!(parser.tls_emulation(), wreq_util::Profile::Firefox135);
        assert_eq!(parser.max_depth(), 2);
    }
}
