//! Wreq-based downloader implementation.
//!
//! Wraps a shared `wreq::Client` behind `Arc` for connection pooling.
//! Extracts cookies from responses and returns [`FetchedPage`] with HTML + cookies.
//!
//! Following **own-arc-shared**: Uses `Arc<Client>` for thread-safe shared ownership
//! of the connection pool. The client is created once and shared across all requests.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tracing::{debug, instrument, warn};
use url::Url;
use wreq::cookie::Jar;
use wreq::header::{HeaderMap, HeaderName, HeaderValue};
use wreq::Client;
use wreq_util::Profile;

use super::{Cookie, DownloadError, Downloader, FetchedPage};
use crate::domain::waf::{is_t2_blocking_status, waf_inspector, InspectionContext};
use crate::error::ErrorClass;
use crate::infrastructure::user_agent::UserAgentCache;

/// Estimated memory cost of a wreq client instance in bytes.
///
/// This accounts for the connection pool, TLS session cache, and internal buffers.
/// Value is approximate — real usage varies by pool size and active connections.
const WREQ_MEMORY_COST: usize = 1_024 * 1_024; // ~1 MB

/// Bytes of a non-2xx response body read for WAF challenge inspection (F-11).
///
/// Bounded on purpose: this read happens on every failing attempt of every crawl, so
/// the cost is failures × attempts × this number. Cloudflare, Akamai and DataDome
/// challenge documents keep their marker prose ("Just a moment…", "Checking your
/// browser", the `cf_chl_*` script prelude) inside the first few KiB, and every T1
/// signature the engine knows is a substring match — so 8 KiB captures all of them
/// while a pathological error page cannot be buffered per attempt. Deliberately far
/// below `max_page_bytes`: this is evidence, not content.
const WAF_SNIFF_MAX_BYTES: u64 = 8 * 1024;

/// What to do when a bounded body read reaches its ceiling.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BoundedRead {
    /// Abort with [`DownloadError::BodyTooLarge`] — the page path, where the operator
    /// configured a hard size cap (FIX-1, #1231 F-12).
    Abort,
    /// Keep the prefix and stop pulling the stream — the WAF sniff path (F-11), where a
    /// body longer than the budget is normal and the prefix is all the evidence needs.
    Truncate,
}

/// Collect a `wreq` header map into the lowercased, single-valued form the WAF
/// inspection boundary expects (REQ-WAF-01).
///
/// This adapter — and not the inspection logic — is what each HTTP stack keeps local,
/// because `domain` must not learn `wreq` types. Duplicate header names collapse with
/// last-value-wins, which is sound here: every WAF control header is single-valued.
fn lowercase_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_lowercase(), v.to_string()))
        })
        .collect()
}
/// Downloader implementation backed by `wreq` with connection pooling.
///
/// The internal `wreq::Client` is shared via `Arc` — all requests reuse the same
/// connection pool, avoiding the per-request client creation anti-pattern.
///
/// # Examples
///
/// ```ignore
/// use webfang_core::infrastructure::downloader::wreq_downloader::WreqDownloader;
/// use webfang_core::infrastructure::downloader::Downloader;
///
/// let downloader = WreqDownloader::new(30, 10, wreq_util::Profile::Chrome145, None, Vec::new(), None, None, 3, 1000, 10000, 50_000_000).unwrap();
/// let page = downloader.fetch(&"https://example.com".parse().unwrap()).await.unwrap();
/// assert_eq!(page.status, 200);
/// ```
pub struct WreqDownloader {
    client: Arc<Client>,
    timeout_secs: u64,
    /// User-Agent pinned by the operator (`--user-agent`, #503).
    ///
    /// Applied once at client-build time so every request — first fetch and
    /// all retries — carries it. When set, the 403 pool-rotation retry is
    /// disabled: the operator asked to be identified exactly as configured.
    pinned_ua: Option<String>,
    max_retries: u32,
    /// Base delay for the exponential backoff applied to retriable failures.
    backoff_base_ms: u64,
    backoff_max_ms: u64,
    /// Decompressed-body cap for page fetches (FIX-1, #1231 F-12). The read
    /// aborts mid-body once the streamed byte count exceeds this value, so
    /// memory stays bounded even against a decompression bomb with a tiny
    /// declared Content-Length.
    max_page_bytes: u64,
    /// Bypass WAF/CAPTCHA classification entirely (`--ignore-waf`, REQ-WAF-07).
    /// Set through [`WreqDownloader::with_ignore_waf`]; `false` enforces detection.
    ignore_waf: bool,
}

impl WreqDownloader {
    /// Create a new WreqDownloader with the given TLS/HTTP2 emulation profile.
    ///
    /// The client is built once and shared via `Arc` for connection pooling.
    /// Pass [`Profile::Chrome145`] for the historical default fingerprint.
    ///
    /// # Arguments
    ///
    /// * `timeout_secs` - Request timeout in seconds
    /// * `connect_timeout_secs` - Connection timeout in seconds
    /// * `tls_emulation` - TLS/HTTP2 fingerprint profile applied to the client
    /// * `user_agent` - Optional pinned User-Agent (#503). Applied at client
    ///   build time via the builder's `user_agent` API, AFTER the emulation
    ///   profile, so it wins over the profile-default UA on the wire. When
    ///   set, the 403 pool-rotation retry is disabled.
    /// * `custom_headers` - Operator headers (`--header`, #890). Applied at
    ///   client build time via `default_headers`, AFTER the emulation
    ///   profile and any pinned UA, so they replace same-named profile
    ///   defaults (case-insensitive per HTTP semantics).
    /// * `accept_language` - Optional Accept-Language value (`--accept-language`,
    ///   #890). Applied AFTER the emulation profile so it wins over the
    ///   profile-default language.
    /// * `initial_cookie_jar` - Pre-seeded wreq cookie store (`--cookie`,
    ///   #890). When provided it replaces the default empty jar, so Static
    ///   and Hybrid-L1 requests carry the operator's cookies from the first
    ///   fetch. The Chromiumoxide L3 path keeps using [`crate::domain::cookie_bridge::CookieBridge`].
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Internal`] if the wreq client cannot be built
    /// or if a custom header name/value is not valid for HTTP.
    // 10 params: the wreq layer's full dependency set (profile, UA, operator
    // headers/cookies, retry backoff) — same pattern as build_fetch_router.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        timeout_secs: u64,
        connect_timeout_secs: u64,
        tls_emulation: Profile,
        user_agent: Option<String>,
        custom_headers: Vec<(String, String)>,
        accept_language: Option<String>,
        initial_cookie_jar: Option<Arc<Jar>>,
        max_retries: u32,
        backoff_base_ms: u64,
        backoff_max_ms: u64,
        max_page_bytes: u64,
    ) -> Result<Self, DownloadError> {
        // Canonical detector seam (Q2): same "auto" as every other subsystem.
        let pool_size = std::cmp::max(
            6,
            crate::domain::budget::detector::system_parallelism().get() - 1,
        );

        // `.emulation(profile)` installs the profile-default headers (including
        // a browser UA) via `default_headers`; a pinned UA must be set AFTER
        // it so `HeaderMap::insert` replaces the profile value (#503).
        let builder = Client::builder().emulation(tls_emulation);
        let builder = match user_agent.as_deref() {
            Some(ua) => builder.user_agent(ua),
            None => builder,
        };
        // #890: operator values are applied as client default headers AFTER
        // the emulation profile. `default_headers` replaces per header name
        // (case-insensitive — HTTP/2 names are lowercase-canonical), so user
        // supplied values win over profile defaults on the wire.
        let mut extra_headers = HeaderMap::new();
        if let Some(lang) = accept_language.as_deref() {
            let value = HeaderValue::from_str(lang).map_err(|e| {
                DownloadError::Internal(format!("invalid --accept-language value {lang:?}: {e}"))
            })?;
            extra_headers.insert(wreq::header::ACCEPT_LANGUAGE, value);
        }
        for (name, value) in &custom_headers {
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                DownloadError::Internal(format!("invalid --header name {name:?}: {e}"))
            })?;
            let value = HeaderValue::from_str(value).map_err(|e| {
                DownloadError::Internal(format!("invalid --header value for {name}: {e}"))
            })?;
            extra_headers.insert(name, value);
        }
        let builder = if extra_headers.is_empty() {
            builder
        } else {
            // #890 observability: WHICH overrides are active (names +
            // count only — never values; cookies and fingerprint headers
            // are credentials/sensitive surface).
            debug!(
                header_names = %extra_headers.keys().map(|k| k.as_str()).collect::<Vec<_>>().join(","),
                custom_header_count = custom_headers.len(),
                accept_language_override = accept_language.is_some(),
                "operator header overrides applied to scrape client"
            );
            builder.default_headers(extra_headers)
        };
        // #890: a pre-seeded jar (`--cookie`) replaces the default empty one
        // so Static/Hybrid-L1 requests carry the operator cookies from the
        // first fetch. `cookie_store(true)` must NOT run afterwards — it
        // would swap in a fresh empty jar.
        let builder = match initial_cookie_jar {
            Some(jar) => {
                // #890 observability: attachment event only — cookie
                // values AND counts live where the jar is seeded
                // (cli/scrape_flow.rs); cookies are credentials.
                debug!("pre-seeded operator cookie jar attached to scrape client");
                builder.cookie_provider(jar)
            },
            None => builder.cookie_store(true),
        };
        let builder = builder
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(connect_timeout_secs))
            .pool_max_idle_per_host(pool_size)
            .pool_idle_timeout(Duration::from_secs(60))
            .gzip(true)
            .brotli(true);
        // SSRF guard (#703) applied through the domain `SsrfGuard` port:
        // default 10-hop limit + stops redirects that target a literal
        // forbidden IP (belt-and-suspenders). Hostname targets — including
        // every redirect hop — are enforced at connect time by the
        // validating DNS resolver the guard installs.
        let client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()
            // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
            .map_err(|e| DownloadError::Internal(format!("failed to build wreq client: {e}")))?;

        debug!(
            pool_size = pool_size,
            timeout_secs = timeout_secs,
            connect_timeout_secs = connect_timeout_secs,
            ua = user_agent.as_deref().unwrap_or("emulation-default"),
            max_retries = max_retries,
            backoff_base_ms = backoff_base_ms,
            backoff_max_ms = backoff_max_ms,
            "WreqDownloader created"
        );

        Ok(Self {
            client: Arc::new(client),
            timeout_secs,
            pinned_ua: user_agent,
            max_retries,
            backoff_base_ms,
            backoff_max_ms,
            max_page_bytes,
            // WAF enforcement is ON by default; `--ignore-waf` opts out through
            // [`WreqDownloader::with_ignore_waf`] at construction time.
            ignore_waf: false,
        })
    }

    /// Opt out of WAF/CAPTCHA classification (`--ignore-waf`, REQ-WAF-07).
    ///
    /// A builder rather than a twelfth `new` parameter on purpose: a positional `bool`
    /// would have rewritten ~20 construction sites (most of them tests) to change one
    /// default, and the flag is genuinely optional behaviour, which a method name states
    /// better than an argument does.
    ///
    /// On the non-2xx path this does NOT rescue the fetch — a challenge body is never
    /// scraped as content. It changes only the classification of the failure: a plain
    /// HTTP error instead of a WAF block, same exit code (F-11).
    #[must_use]
    pub fn with_ignore_waf(mut self, ignore_waf: bool) -> Self {
        self.ignore_waf = ignore_waf;
        self
    }

    /// Create a WreqDownloader from an existing `wreq::Client`.
    ///
    /// Useful when you need custom client configuration beyond the defaults.
    pub fn from_client(client: Client, timeout_secs: u64, _connect_timeout_secs: u64) -> Self {
        Self {
            client: Arc::new(client),
            timeout_secs,
            pinned_ua: None,
            max_page_bytes: crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            ignore_waf: false,
        }
    }

    /// Extract cookies from a wreq response.
    fn extract_cookies(url: &Url, response: &wreq::Response) -> Vec<Cookie> {
        let mut cookies = Vec::new();

        // Extract cookies from the cookie store via the response cookies
        for cookie in response.cookies() {
            cookies.push(Cookie {
                name: cookie.name().to_string(),
                value: cookie.value().to_string(),
                domain: cookie.domain().unwrap_or("").to_string(),
                path: cookie.path().unwrap_or("/").to_string(),
                http_only: cookie.http_only(),
                secure: cookie.secure(),
            });
        }

        // Also extract Set-Cookie headers for cookies not in the store
        let set_cookie_headers = response.headers().get_all("set-cookie");
        let existing_names: std::collections::HashSet<_> =
            cookies.iter().map(|c| c.name.clone()).collect();

        for header_value in set_cookie_headers {
            if let Ok(value_str) = header_value.to_str() {
                // Parse basic cookie fields from Set-Cookie header
                if let Some(cookie) = parse_set_cookie(value_str, url) {
                    if !existing_names.contains(&cookie.name) {
                        cookies.push(cookie);
                    }
                }
            }
        }

        cookies
    }
}

/// Parse a Set-Cookie header value into a Cookie struct.
fn parse_set_cookie(header: &str, url: &Url) -> Option<Cookie> {
    let parts: Vec<&str> = header.split(';').collect();
    if parts.is_empty() {
        return None;
    }

    let name_value = parts[0].trim();
    let pos = name_value.find('=')?;
    let (name, value) = (
        name_value[..pos].trim().to_string(),
        name_value[pos + 1..].trim().to_string(),
    );

    if name.is_empty() {
        return None;
    }

    let mut domain = url.host_str().unwrap_or("").to_string();
    let mut path = "/".to_string();
    let mut http_only = false;
    let mut secure = false;

    for part in &parts[1..] {
        let part = part.trim().to_lowercase();
        if let Some(val) = part.strip_prefix("domain=") {
            domain = val.trim().to_string();
        } else if let Some(val) = part.strip_prefix("path=") {
            path = val.trim().to_string();
        } else if part == "httponly" {
            http_only = true;
        } else if part == "secure" {
            secure = true;
        }
    }

    Some(Cookie {
        name,
        value,
        domain,
        path,
        http_only,
        secure,
    })
}

/// Parse the `Retry-After` header (integer seconds) into milliseconds.
///
/// Returns the delay in ms, defaulting to 1000ms if the header is absent or unparseable.
fn parse_retry_after_ms(response: &wreq::Response, max_ms: u64) -> u64 {
    let raw_ms = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(1)
        .saturating_mul(1000);
    raw_ms.min(max_ms)
}

impl WreqDownloader {
    /// Describe the effective User-Agent of a request for tracing (#503):
    /// the request-level override (403 rotation), else the pinned value,
    /// else the emulation-profile default.
    fn effective_ua<'a>(&'a self, request_ua: Option<&'a str>) -> &'a str {
        request_ua
            .or(self.pinned_ua.as_deref())
            .unwrap_or("emulation-default")
    }

    #[instrument(skip(self), fields(url = %url, ua = %self.effective_ua(user_agent)))]
    async fn send_request(
        &self,
        url: &Url,
        user_agent: Option<&str>,
    ) -> Result<wreq::Response, DownloadError> {
        let builder = self.client.get(url.as_str());
        let builder = match user_agent {
            Some(ua) => builder.header("User-Agent", ua),
            None => builder,
        };
        builder.send().await.map_err(|e| {
            if e.is_timeout() {
                DownloadError::Timeout(self.timeout_secs)
            } else {
                DownloadError::from(e)
            }
        })
    }

    /// Calculate the exponential backoff delay for a given attempt, capped at
    /// `backoff_max_ms`.
    fn backoff_delay_ms(&self, attempt: u32) -> u64 {
        self.backoff_base_ms
            .saturating_mul(2_u64.saturating_pow(attempt))
            .min(self.backoff_max_ms)
    }

    /// Sleep before the next attempt — skipped on the final one, where the
    /// loop is about to exit and the delay would only add dead latency.
    async fn sleep_before_retry(&self, attempt: u32, delay_ms: u64) {
        if attempt < self.max_retries {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
    }

    /// Consume a successful response into a [`FetchedPage`].
    async fn build_page(
        &self,
        response: wreq::Response,
        url: &Url,
    ) -> Result<FetchedPage, DownloadError> {
        let status = response.status().as_u16();

        // Extract cookies before consuming the response body
        let cookies = Self::extract_cookies(url, &response);

        // Extract the final URL after redirects
        let final_url = Url::parse(&response.uri().to_string())
            .map_err(|e| DownloadError::InvalidUrl(e.to_string()))?;

        // Capture response headers (lowercased keys) before consuming the body.
        let headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();

        let html = self.read_body_capped(response).await?;

        debug!(
            "Fetched {} ({} bytes, {} cookies)",
            final_url,
            html.len(),
            cookies.len()
        );

        Ok(FetchedPage {
            url: final_url,
            html,
            status,
            headers,
            cookies,
        })
    }

    /// Read the response body with a HARD cap on the decompressed byte count
    /// (FIX-1, #1231 F-12).
    ///
    /// Replaces the unbounded `response.text()`: the body is consumed as a
    /// stream of already-decompressed chunks and the read aborts as soon as
    /// the accumulated size exceeds the configured `max_page_bytes`, so
    /// memory stays bounded at ~cap + one chunk regardless of the wire
    /// content (the audit's gzip bomb inflated 60 MB from a tiny
    /// Content-Length). Charset handling mirrors wreq's `text_with_charset`:
    /// the Content-Type charset param wins, UTF-8 as fallback.
    async fn read_body_capped(&self, response: wreq::Response) -> Result<String, DownloadError> {
        self.read_body_bounded(response, self.max_page_bytes, BoundedRead::Abort)
            .await
    }

    /// Read at most [`WAF_SNIFF_MAX_BYTES`] of a response body for challenge
    /// inspection (F-11).
    ///
    /// Truncates rather than failing: unlike a page fetch, exceeding the budget here is
    /// the expected case — the caller only wants the prefix that carries marker prose.
    async fn read_body_snippet(&self, response: wreq::Response) -> Result<String, DownloadError> {
        self.read_body_bounded(response, WAF_SNIFF_MAX_BYTES, BoundedRead::Truncate)
            .await
    }

    /// Shared bounded body reader — the one place that streams, caps, decodes and
    /// consumes a `wreq::Response`. Both the page path and the WAF sniff path route
    /// through it so a fix to one cannot silently miss the other (F-11).
    ///
    /// `on_overflow` selects the only behavioural difference between them.
    async fn read_body_bounded(
        &self,
        response: wreq::Response,
        limit: u64,
        on_overflow: BoundedRead,
    ) -> Result<String, DownloadError> {
        use futures::StreamExt;

        // Charset from Content-Type BEFORE the body is consumed (headers stay
        // readable until the body stream is taken).
        let content_type = response
            .headers()
            .get(wreq::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value.split(';').find_map(|part| {
                    let part = part.trim();
                    part.strip_prefix("charset=")
                        .map(|c| c.trim_matches('"').trim().to_ascii_lowercase())
                })
            })
            .unwrap_or_else(|| "utf-8".to_string());

        let limit_len = usize::try_from(limit).unwrap_or(usize::MAX);
        let mut stream = response.bytes_stream();
        let mut buf = bytes::BytesMut::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(DownloadError::from)?;
            if buf.len().saturating_add(chunk.len()) > limit_len {
                match on_overflow {
                    BoundedRead::Abort => {
                        // The outer fetch span already carries the URL; the event only
                        // needs the machine-readable cap facts.
                        warn!(
                            limit = limit,
                            "response body exceeded the page size cap; aborting read"
                        );
                        return Err(DownloadError::BodyTooLarge { limit });
                    },
                    BoundedRead::Truncate => {
                        // Keep the prefix that fits and stop pulling the stream: the rest
                        // of the body is never read, so an oversized error page costs at
                        // most the sniff budget.
                        buf.extend_from_slice(&chunk[..limit_len.saturating_sub(buf.len())]);
                        break;
                    },
                }
            }
            buf.extend_from_slice(&chunk);
        }

        let (text, _, _) = encoding_rs::Encoding::for_label(content_type.as_bytes())
            .unwrap_or(encoding_rs::UTF_8)
            .decode(&buf);
        Ok(text.into_owned())
    }

    #[instrument(
        skip(self),
        fields(
            url = %url,
            // D5: stable identity of the shared pooled `Client` (Arc inner ptr).
            // Constant across fetches => observable proof of connection-pool reuse
            // (no silent re-handshake per request). See MAPA item 7.
            client_id = %format!("{:p}", Arc::as_ptr(&self.client))
        )
    )]
    async fn fetch_inner(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!("Fetching URL: {}", url);

        let mut last_status: u16 = 0;
        let mut last_error: Option<DownloadError> = None;

        for attempt in 0..=self.max_retries {
            let response = match self.send_request(url, None).await {
                Ok(res) => res,
                Err(dl_err) => {
                    // Single classification rule (FIX-1, #1236): retry
                    // everything the domain classifier deems transient, surface
                    // PermanentFatal immediately. Request timeouts are
                    // TransientBackoff (F-08, #1231): the most common transient
                    // failure in crawling (slow-not-dead peer), and the
                    // configured timeout still caps EACH attempt, so the retry
                    // budget of `max_retries` x `timeout_secs` is exactly what
                    // the operator asked for. Builder-class invalid requests
                    // (unsupported scheme, malformed URL) map to InvalidUrl ->
                    // PermanentFatal and fail after a single attempt (F-09,
                    // #1236); mid-body transients (Io::ConnectionReset /
                    // UnexpectedEof) stay retriable (#649 mid-body transient
                    // fix).
                    if matches!(dl_err.classify(), ErrorClass::PermanentFatal) {
                        return Err(dl_err);
                    }
                    // The "retrying" event only fires when a retry will
                    // actually happen: on the last attempt the loop ends and
                    // the stored error surfaces without a wasted backoff.
                    let will_retry = attempt < self.max_retries;
                    if will_retry {
                        warn!(
                            attempt = attempt,
                            max_retries = self.max_retries,
                            error = %dl_err,
                            "Transport failure fetching {url} — retrying"
                        );
                    }
                    last_error = Some(dl_err);
                    if will_retry {
                        self.sleep_before_retry(attempt, self.backoff_delay_ms(attempt))
                            .await;
                    }
                    continue;
                },
            };

            last_status = response.status().as_u16();
            last_error = None;

            if response.status().is_success() {
                return self.build_page(response, url).await;
            }

            // ── WAF inspection on non-2xx responses (F-11) ─────────────────────────
            //
            // The tiered inspector used to see only successful responses, so the ordinary
            // Cloudflare shape — 403/503 carrying `cf-mitigated: challenge` — surfaced as a
            // generic HTTP error and `--ignore-waf` looked inert on it. The engine is
            // already status-aware (T1 Challenge blocks at any status, T2 Fingerprint needs
            // a WAF status — see `domain::waf`), so this was purely a missing call site,
            // not a detection gap.
            //
            // Placement matters: it runs BEFORE the rotated-UA request and before the
            // 429/5xx backoff, so a confirmed challenge costs zero retries. A WAF challenge
            // does not clear by rotating a User-Agent, and #1236's amplification lesson says
            // a request that cannot succeed should not be repeated.
            //
            // Retry-After is captured BEFORE anything reads the body: `read_body_snippet`
            // consumes the response, and the 429 branch below still needs the server's
            // requested delay or #1231's backoff silently degrades.
            let mut retry_after_ms =
                (last_status == 429).then(|| parse_retry_after_ms(&response, self.backoff_max_ms));

            if !self.ignore_waf {
                let headers = lowercase_headers(response.headers());
                let ctx = InspectionContext::from_lowercase_headers(last_status, &headers, false);

                // Phase 1 — headers only, no read at all. Control headers such as
                // `cf-mitigated` are T2 evidence and block on their own even with an empty
                // body (RES-01), which is exactly how a mitigating edge answers.
                let verdict = waf_inspector().inspect("", &ctx);
                let verdict = if verdict.is_blocked || !is_t2_blocking_status(ctx.status) {
                    verdict
                } else {
                    // Phase 2 — T1 marker prose, but only on a status a WAF actually
                    // answers with. T2 never blocks outside that set, so a plain 404 would
                    // pay a body read for a verdict that cannot change; a 404 serving
                    // challenge prose stays unclassified by design, and is reported as the
                    // HTTP error it is.
                    let snippet = self.read_body_snippet(response).await?;
                    waf_inspector().inspect(&snippet, &ctx)
                };

                if verdict.is_blocked {
                    let chain = verdict.evidence_chain();
                    warn!(
                        url = %url,
                        status = last_status,
                        evidences = verdict.evidences.len(),
                        "WAF/CAPTCHA challenge detected on a non-2xx response"
                    );
                    return Err(DownloadError::WafChallenge(chain));
                }
            }

            // 403: one implicit retry with rotated User-Agent (mirrors HttpClient).
            // Pinned UA (#503): the operator asked to be identified exactly as
            // configured, so pool rotation is skipped and the 403 falls through
            // to normal terminal-error handling.
            //
            // IMPORTANT: The rotated-UA retry MUST capture its response and status
            // so the unified retry loop can correctly handle 429/5xx that the
            // rotated attempt may return. The old helper `retry_with_rotated_ua`
            // discarded the non-2xx response, causing 403→429→200 sequences to
            // fail because the 429 was silently consumed.
            if last_status == 403 && attempt == 0 && self.pinned_ua.is_none() {
                let agents = UserAgentCache::fallback_agents();
                let rotated_ua = agents.get(1).map(String::as_str);
                warn!("403 Forbidden from {url} — retrying with rotated User-Agent");
                match self.send_request(url, rotated_ua).await {
                    Ok(res) if res.status().is_success() => {
                        return self.build_page(res, url).await;
                    },
                    Ok(res) => {
                        // Rotated retry returned non-2xx (e.g., a second
                        // 403, 429, 500). The rotation CONSUMES the attempt:
                        // fall through to the unified 429/5xx-or-terminal
                        // handling below instead of `continue`-ing back to
                        // the loop top. A bare `continue` re-enters the loop
                        // and fires another default-UA request for the same
                        // failure (#1430: always-403 cost 3 requests —
                        // default, rotated, default — instead of 2,
                        // amplification against WAFs), and skips the backoff
                        // sleep a rotated 429/5xx owes. The `attempt == 0`
                        // guard above stays the single "no rotation spent
                        // yet" gate — no parallel flag.
                        last_status = res.status().as_u16();
                        retry_after_ms = (last_status == 429)
                            .then(|| parse_retry_after_ms(&res, self.backoff_max_ms));
                    },
                    Err(e) => return Err(e),
                }
            }

            // Unified retry for 429 (rate limit) and 5xx (server error, #649).
            if last_status == 429 || (500..=599).contains(&last_status) {
                let delay_ms = if last_status == 429 {
                    // BUG 6 fix: use max(Retry-After, exponential_backoff) instead of fixed constant.
                    // parse_retry_after_ms returns the server's requested delay (capped at backoff_max_ms).
                    // backoff_delay_ms returns exponential delay (also capped).
                    // max() ensures we never retry faster than the server asked,
                    // but also never slower than our own exponential strategy.
                    std::cmp::max(retry_after_ms.unwrap_or(0), self.backoff_delay_ms(attempt))
                } else {
                    self.backoff_delay_ms(attempt) // 5xx already correct
                };
                warn!(
                    attempt = attempt,
                    max_retries = self.max_retries,
                    status = last_status,
                    delay_ms = delay_ms,
                    "Retrying {url} after status {last_status}"
                );
                self.sleep_before_retry(attempt, delay_ms).await;
                continue;
            }

            // Terminal error (4xx and anything else non-retriable).
            return Err(DownloadError::Http {
                status: last_status,
                message: format!("HTTP {last_status}"),
            });
        }

        // Retries exhausted — surface the LAST observed status, not a hardcoded
        // one (#649 Bug 5): a run that started at 429 and ended at 500 must
        // report 500.
        Err(last_error.unwrap_or(DownloadError::Http {
            status: last_status,
            message: format!("retries exhausted at status {last_status}"),
        }))
    }
}

impl Downloader for WreqDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(self.fetch_inner(url))
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        WREQ_MEMORY_COST
    }
}

#[cfg(test)]
mod test_support {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Mount a `GET /` mock returning `body` (200), then fetch it via a fresh
    /// `WreqDownloader` and assert the basics (ok, status 200, body matches).
    /// Returns the fetched `FetchedPage` so callers can add extra assertions.
    #[allow(dead_code)]
    pub(super) async fn fetch_mock_get(body: &str) -> FetchedPage {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(body))
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_ok());
        let page = result.unwrap();
        assert_eq!(page.status, 200);
        assert_eq!(page.html, body);
        page
    }
}

#[cfg(test)]
#[cfg(not(miri))] // wreq uses boring-sys2 FFI (unsupported by Miri)
mod tests {
    use super::*;

    #[test]
    fn test_wreq_downloader_creation() {
        let downloader = WreqDownloader::new(
            30,
            10,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        assert!(!downloader.supports_interactions());
        assert_eq!(downloader.memory_cost(), WREQ_MEMORY_COST);
    }

    #[test]
    fn test_wreq_downloader_honors_tls_profile() {
        // The constructor must accept every catalog profile and build a client
        // with it (triangulation: the parameter reaches the builder instead of
        // a hardcoded default).
        for profile in [Profile::Chrome145, Profile::Chrome131, Profile::Firefox135] {
            let downloader = WreqDownloader::new(
                30,
                10,
                profile,
                None,
                Vec::new(),
                None,
                None,
                3,
                1000,
                10000,
                crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
            )
            .unwrap_or_else(|e| panic!("client must build for profile {profile:?}: {e}"));
            assert!(!downloader.supports_interactions());
        }
    }

    #[test]
    fn test_wreq_downloader_from_client() {
        let client = Client::builder()
            .emulation(Profile::Chrome145)
            .build()
            .unwrap();
        let downloader = WreqDownloader::from_client(client, 60, 15);
        assert!(!downloader.supports_interactions());
    }

    /// SSRF choke-point wiring proof (#1060): the client `WreqDownloader::new`
    /// actually builds must carry the guard obtained from the `SsrfGuard` port.
    /// `localhost` resolves to loopback through getaddrinfo with no network
    /// dependency, so the connect attempt must be rejected by the validating
    /// resolver — not by the (absent) listener on port 9.
    #[tokio::test]
    async fn downloader_client_enforces_ssrf_guard_from_the_port() {
        // Env hermeticity (#926): the escape hatch is captured at client-build
        // time, so clearing it must be serialized against siblings that set it.
        // `EnvGuard` holds the shared process-env lock and restores on drop.
        let _env = webfang_test_utils::EnvGuard::clean(&[
            crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
        ]);
        let downloader = WreqDownloader::new(
            30,
            10,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();

        let err = downloader
            .client
            .get("http://localhost:9/")
            .send()
            .await
            .expect_err("hostname resolving to loopback must fail at connect");
        assert!(
            format!("{err:?}").contains("ForbiddenResolutionError"),
            "failure must come from the SSRF resolver, not the network: {err:?}"
        );
    }

    #[test]
    fn test_parse_set_cookie_basic() {
        let header = "session=abc123; Path=/; HttpOnly; Secure";
        let url: Url = "https://example.com".parse().unwrap();
        let cookie = parse_set_cookie(header, &url).unwrap();

        assert_eq!(cookie.name, "session");
        assert_eq!(cookie.value, "abc123");
        assert_eq!(cookie.domain, "example.com");
        assert_eq!(cookie.path, "/");
        assert!(cookie.http_only);
        assert!(cookie.secure);
    }

    #[test]
    fn test_parse_set_cookie_custom_domain() {
        let header = "token=xyz; Domain=.api.example.com; Path=/api";
        let url: Url = "https://example.com".parse().unwrap();
        let cookie = parse_set_cookie(header, &url).unwrap();

        assert_eq!(cookie.name, "token");
        assert_eq!(cookie.value, "xyz");
        assert_eq!(cookie.domain, ".api.example.com");
        assert_eq!(cookie.path, "/api");
        assert!(!cookie.http_only);
        assert!(!cookie.secure);
    }

    #[test]
    fn test_parse_set_cookie_empty_name() {
        let header = "=value; Path=/";
        let url: Url = "https://example.com".parse().unwrap();
        assert!(parse_set_cookie(header, &url).is_none());
    }

    #[test]
    fn test_parse_set_cookie_no_equals() {
        let header = "invalid";
        let url: Url = "https://example.com".parse().unwrap();
        assert!(parse_set_cookie(header, &url).is_none());
    }

    #[test]
    fn test_parse_set_cookie_empty_header() {
        let url: Url = "https://example.com".parse().unwrap();
        assert!(parse_set_cookie("", &url).is_none());
    }

    #[tokio::test]
    async fn test_fetch_uses_mock_server() {
        let expected_body = "<html><body><h1>mock</h1></body></html>";
        super::test_support::fetch_mock_get(expected_body).await;
    }
}

#[cfg(test)]
#[cfg(not(miri))]
mod wiremock_tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Match, Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_fetch_200_returns_body() {
        let expected_body = "<html><body>Hello World</body></html>";
        let page = super::test_support::fetch_mock_get(expected_body).await;
        assert_eq!(page.html, expected_body);
    }

    #[tokio::test]
    async fn test_fetch_404_returns_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/notfound"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = format!("{}/notfound", mock_server.uri()).parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            DownloadError::Http { status: 404, .. }
        ));
    }

    #[tokio::test]
    async fn test_fetch_extracts_cookies() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html></html>")
                    .insert_header("set-cookie", "session=abc123; Path=/; HttpOnly"),
            )
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_ok());

        let page = result.unwrap();
        assert_eq!(page.status, 200);
        assert!(!page.cookies.is_empty());

        let cookie = &page.cookies[0];
        assert_eq!(cookie.name, "session");
        assert_eq!(cookie.value, "abc123");
        assert!(cookie.http_only);
    }

    #[tokio::test]
    async fn test_fetch_returns_final_url() {
        // The SSRF redirect guard (#703) stops redirects targeting a literal
        // forbidden IP, and wiremock binds 127.0.0.1 — lift the guard for this
        // redirect-flow test before the client is built.
        let _guard = webfang_test_utils::EnvGuard::with(&[(
            crate::infrastructure::ssrf::DISABLE_REDIRECT_GUARD_ENV,
            "1",
        )]);
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(ResponseTemplate::new(301).insert_header("location", "/target"))
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/target"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = format!("{}/redirect", mock_server.uri()).parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_ok());

        let page = result.unwrap();
        assert!(page.url.as_str().contains("/target"));
    }

    /// SSRF redirect guard (#703): a redirect whose `Location` is a literal
    /// forbidden IP must be stopped even when the entry URL itself passed
    /// validation. The redirect response surfaces as a terminal HTTP error and
    /// the target is never requested (`expect(0)` proves wiremock saw no hit).
    #[tokio::test]
    async fn test_redirect_to_forbidden_literal_ip_is_stopped() {
        // Defensive under shared-process harnesses: the escape hatch must be
        // unset for this process so the guard is active. (nextest isolates
        // each test in its own process, so this is a no-op there.)
        let _guard = webfang_test_utils::EnvGuard::clean(&[
            crate::infrastructure::ssrf::DISABLE_REDIRECT_GUARD_ENV,
        ]);
        let mock_server = MockServer::start().await;

        // Location points at a different loopback literal — forbidden by the
        // guard, unreachable in practice, and never requested.
        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(
                ResponseTemplate::new(301).insert_header("location", "http://127.0.0.2:9/target"),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/target"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .expect(0)
            .mount(&mock_server)
            .await;

        // Fresh client built in this process: the guard env hatch stays unset.
        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = format!("{}/redirect", mock_server.uri()).parse().unwrap();

        let result = downloader.fetch(&url).await;
        match result {
            Err(DownloadError::Http { status, .. }) => assert_eq!(status, 301),
            other => panic!("expected redirect to be stopped, got: {other:?}"),
        }
    }

    // ------------------------------------------------------------------
    // Pinned User-Agent (#503)
    // ------------------------------------------------------------------

    /// Wire-level proof that a pinned UA wins over the emulation-profile
    /// default UA: the TLS/HTTP2 fingerprint (Chrome145) is fully enabled,
    /// yet the server receives exactly the pinned value. The mock only
    /// matches `QA-Bot/9.9` — if the profile UA leaked onto the wire the
    /// request would 404 and the fetch would fail.
    #[tokio::test]
    async fn pinned_user_agent_beats_emulation_profile_on_the_wire() {
        const PINNED_UA: &str = "QA-Bot/9.9";
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("User-Agent", PINNED_UA))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            Some(PINNED_UA.to_string()),
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let page = downloader
            .fetch(&url)
            .await
            .expect("fetch succeeds only if the pinned UA matched on the wire");
        assert_eq!(page.status, 200);

        // Second proof: the server-side record shows exactly the pinned UA.
        let requests = mock_server
            .received_requests()
            .await
            .expect("server records requests");
        let ua = requests
            .first()
            .expect("one request recorded")
            .headers
            .get("user-agent")
            .expect("User-Agent header present");
        assert_eq!(
            ua.to_str().expect("User-Agent must be valid ASCII"),
            PINNED_UA
        );
    }

    // ------------------------------------------------------------------
    // #890 — operator headers, Accept-Language, and seeded cookies
    // ------------------------------------------------------------------

    /// Wire-level proof that an operator custom header (`--header`, #890)
    /// reaches the outgoing scrape request even though the Chrome145
    /// emulation profile does not send it by default.
    #[tokio::test]
    async fn custom_header_reaches_the_wire() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .and(RawHeaderEquals {
                name: "x-wftest",
                value: "hola".to_string(),
            })
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            vec![("X-WFTest".to_string(), "hola".to_string())],
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let page = downloader
            .fetch(&url)
            .await
            .expect("fetch succeeds only if the custom header matched on the wire");
        assert_eq!(page.status, 200);
    }

    /// Wire-level proof that the configured Accept-Language (`
    /// --accept-language`, #890) replaces the emulation-profile default on
    /// the wire instead of being silently dropped (#890 reproduction showed
    /// `es-AR` arriving as the profile's `en-US,en;q=0.9`).
    #[tokio::test]
    async fn accept_language_replaces_profile_default_on_the_wire() {
        const LANG: &str = "es-AR,es;q=0.9";
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .and(RawHeaderEquals {
                name: "accept-language",
                value: LANG.to_string(),
            })
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            Some(LANG.to_string()),
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let page = downloader
            .fetch(&url)
            .await
            .expect("fetch succeeds only if the configured language matched on the wire");
        assert_eq!(page.status, 200);
    }

    /// Malformed operator input (#890, review M1): an invalid header NAME must
    /// fail LOUDLY at client build time with a typed internal error naming the
    /// offending header — never a silent drop (the same silent-drop class #890
    /// exists to eliminate).
    #[tokio::test]
    async fn invalid_custom_header_name_fails_loudly_at_build() {
        let error = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            vec![("bad header name\n".to_string(), "v".to_string())],
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .err()
        .expect("newline is not a valid HTTP header name");
        match error {
            DownloadError::Internal(msg) => {
                assert!(msg.contains("invalid --header name"), "got: {msg}");
            },
            other => panic!("expected DownloadError::Internal, got: {other:?}"),
        }
    }

    /// Malformed operator input (#890, review M1): an invalid header VALUE and
    /// an invalid Accept-Language must both fail loudly at client build time.
    #[tokio::test]
    async fn invalid_header_value_and_language_fail_loudly_at_build() {
        let value_error = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            vec![("X-Ok".to_string(), "bad\nvalue".to_string())],
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .err()
        .expect("newline is not a valid HTTP header value");
        assert!(
            matches!(value_error, DownloadError::Internal(ref m) if m.contains("invalid --header value")),
            "got: {value_error:?}"
        );

        let lang_error = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            Some("es\u{0}-AR".to_string()),
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .err()
        .expect("NUL is not a valid Accept-Language value");
        assert!(
            matches!(lang_error, DownloadError::Internal(ref m) if m.contains("invalid --accept-language")),
            "got: {lang_error:?}"
        );
    }

    /// Wire-level proof that a pre-seeded cookie jar (`--cookie`, #890) is
    /// carried by Static/Hybrid-L1 wreq requests from the very first fetch.
    #[tokio::test]
    async fn seeded_cookie_jar_carries_operator_cookies_on_the_wire() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("cookie", "k=v"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html></html>"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let jar = Jar::default();
        jar.add("k=v", mock_server.uri().as_str());
        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            Some(std::sync::Arc::new(jar)),
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let page = downloader
            .fetch(&url)
            .await
            .expect("fetch succeeds only if the seeded cookie reached the wire");
        assert_eq!(page.status, 200);
    }

    /// Exact-match matcher for a single raw header value.
    ///
    /// Unlike `wiremock::matchers::header`, it does NOT comma-split the
    /// received value: the fallback pool agents embed commas
    /// ("(KHTML, like Gecko)"), which the built-in exact matcher splits into
    /// two values and therefore never matches.
    struct RawHeaderEquals {
        name: &'static str,
        value: String,
    }

    impl Match for RawHeaderEquals {
        fn matches(&self, request: &wiremock::Request) -> bool {
            request
                .headers
                .get_all(self.name)
                .iter()
                .any(|v| v.to_str().is_ok_and(|s| s == self.value))
        }
    }

    /// Issue #503 policy: a pinned UA disables the 403 pool-agent rotation.
    /// The first 403 surfaces as a terminal error (no retry under a rotated
    /// identity), and the next fetch still carries the pinned UA.
    #[tokio::test]
    async fn pinned_user_agent_disables_403_rotation() {
        const PINNED_UA: &str = "QA-Bot/9.9";
        let mock_server = MockServer::start().await;
        let pool_agent = UserAgentCache::fallback_agents().swap_remove(1);

        // First hit: 403 for the pinned UA (consumed once, then falls through).
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("User-Agent", PINNED_UA))
            .respond_with(ResponseTemplate::new(403))
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        // Catches a rotated retry — must remain unmatched (expect(0)).
        Mock::given(method("GET"))
            .and(path("/"))
            .and(RawHeaderEquals {
                name: "user-agent",
                value: pool_agent,
            })
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>rotated</html>"))
            .expect(0)
            .mount(&mock_server)
            .await;

        // Second hit: 200, but only for the pinned UA.
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("User-Agent", PINNED_UA))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>still pinned</html>"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            Some(PINNED_UA.to_string()),
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        // First fetch: 403 is terminal — rotation is disabled when pinned.
        let err = downloader
            .fetch(&url)
            .await
            .expect_err("403 must surface as an error when the UA is pinned");
        assert!(
            matches!(err, DownloadError::Http { status: 403, .. }),
            "expected terminal Http 403, got: {err:?}"
        );

        // Second fetch: still carries the pinned UA (only the pinned mock
        // answers now), proving no identity drift after the 403.
        let page = downloader
            .fetch(&url)
            .await
            .expect("second fetch succeeds with the pinned UA");
        assert_eq!(page.status, 200);
        assert_eq!(page.html, "<html>still pinned</html>");
    }

    // ------------------------------------------------------------------
    // Network resilience (#649)
    // ------------------------------------------------------------------

    /// 5xx must trigger the unified retry loop (1 initial + 3 retries), and
    /// exhaustion must report the LAST observed status — not a hardcoded 429
    /// (#649 Bugs 2 & 5). One server exercises both: first reply 429, then 500.
    #[tokio::test]
    async fn test_unified_retry_fires_and_reports_last_status() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        let server = MockServer::start().await;
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_clone_if = Arc::clone(&counter);

        Mock::given(method("GET"))
            .respond_with(move |_req: &wiremock::Request| {
                let count = counter_clone_if.fetch_add(1, Ordering::SeqCst);
                if count < 2 {
                    ResponseTemplate::new(429).insert_header("retry-after", "0")
                } else {
                    ResponseTemplate::new(500)
                }
            })
            .mount(&server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1,
            5,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .expect("client builds");
        let url: Url = format!("{}/", server.uri()).parse().expect("valid url");

        // 2×429 + 2×500 = 4 requests; the 5xx half proves Bug 2, the final
        // 500 status proves Bug 5 (last observed status, not hardcoded 429).
        match downloader.fetch(&url).await {
            Err(DownloadError::Http { status: 500, .. }) => {},
            other => panic!("Expected status 500 after 429→500 run, got {other:?}"),
        }
        assert_eq!(
            counter.load(Ordering::SeqCst),
            4,
            "Expected 2×429 + 2×500 = 4 requests"
        );
    }

    /// Unpinned baseline: the pre-#503 rotation behavior is preserved —
    /// a 403 triggers one retry with the pool agent and succeeds.
    #[tokio::test]
    async fn unpinned_user_agent_rotates_pool_agent_on_403() {
        let mock_server = MockServer::start().await;
        let pool_agent = UserAgentCache::fallback_agents().swap_remove(1);

        // First hit: 403 regardless of UA (consumed once).
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(403))
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;

        // The rotated retry must arrive with the pool agent.
        Mock::given(method("GET"))
            .and(path("/"))
            .and(RawHeaderEquals {
                name: "user-agent",
                value: pool_agent,
            })
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>rotated</html>"))
            .expect(1)
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let page = downloader
            .fetch(&url)
            .await
            .expect("rotated retry succeeds for unpinned downloads");
        assert_eq!(page.status, 200);
        assert_eq!(page.html, "<html>rotated</html>");
    }

    /// Issue #1430: an always-403 origin must cost EXACTLY two requests —
    /// one with the default UA, one with the rotated pool agent — and the
    /// final error must preserve the last observed status (403).
    /// The rotated retry consumes the attempt: it falls through to the
    /// terminal handling instead of `continue`-ing back to the loop top
    /// (which fired a third, default-UA request against the WAF).
    #[tokio::test]
    async fn always_403_costs_exactly_two_requests_and_reports_403() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};

        let mock_server = MockServer::start().await;
        let pool_agent = UserAgentCache::fallback_agents().swap_remove(1);
        let hits = Arc::new(AtomicUsize::new(0));
        let uas: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let hits_clone = Arc::clone(&hits);
        let uas_clone = Arc::clone(&uas);

        Mock::given(method("GET"))
            .respond_with(move |req: &wiremock::Request| {
                hits_clone.fetch_add(1, Ordering::SeqCst);
                let ua = req
                    .headers
                    .get("user-agent")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("<missing>")
                    .to_string();
                uas_clone.lock().unwrap().push(ua);
                ResponseTemplate::new(403)
            })
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            1000,
            10000,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        match downloader.fetch(&url).await {
            Err(DownloadError::Http { status: 403, .. }) => {},
            other => panic!("Expected terminal Http 403, got {other:?}"),
        }
        assert_eq!(
            hits.load(Ordering::SeqCst),
            2,
            "always-403 must cost exactly 2 requests (default + rotated)"
        );
        let uas = uas.lock().unwrap();
        assert_eq!(
            uas.len(),
            2,
            "expected one default-UA and one rotated-UA request"
        );
        assert_ne!(
            uas[0], pool_agent,
            "first request must carry the default UA, got: {}",
            uas[0]
        );
        assert_eq!(
            uas[1], pool_agent,
            "second request must carry the rotated pool agent"
        );
    }
    // ------------------------------------------------------------------
    // FIX-1 (#1231 F-08, #1236 F-09): retry classification contract tests.
    // The retry loop must honor DownloadError::classify() for the whole
    // transient family: request timeouts are retriable (the most common
    // transient failure in crawling), while builder-class errors (invalid
    // request: unsupported scheme, malformed URL) are permanent and must
    // surface after a SINGLE attempt.
    // ------------------------------------------------------------------

    /// F-08: a request that times out is RETRIED and recovery is served.
    /// wiremock: first /slow hit sleeps past the client timeout (served
    /// once, `.up_to_n_times(1)`), subsequent /slow hits respond
    /// instantly. Asserts exactly 2 outbound requests and a successful
    /// page — the timeout retry fired and its result was used.
    #[tokio::test]
    async fn timeout_is_retried_and_recovery_is_served() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/slow"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("late")
                    .set_delay(Duration::from_millis(1500)),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_body_string("fast"))
            .mount(&server)
            .await;

        // timeout 1s so the first /slow hit (1.5s) trips it; short backoff
        // keeps the test fast.
        let dl = WreqDownloader::new(
            1,
            1,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            2,
            10,
            50,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .expect("test downloader builds");

        let slow_url: Url = format!("{}/slow", server.uri())
            .parse()
            .expect("server uri parses");
        let page = dl
            .fetch(&slow_url)
            .await
            .expect("retry after timeout must recover");
        assert_eq!(page.html, "fast");
        let hits = server
            .received_requests()
            .await
            .expect("received requests")
            .len();
        assert_eq!(
            hits, 2,
            "timeout retry must produce exactly one re-request, got {hits}"
        );
    }

    /// F-09: a builder-class error (unsupported scheme) must NOT be
    /// retried — single attempt, immediate permanent error.
    #[tokio::test]
    async fn unsupported_scheme_is_not_retried() {
        let dl = WreqDownloader::new(
            5,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            10,
            50,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .expect("test downloader builds");

        let url = url::Url::parse("ftp://example.com/x").expect("parseable scheme");
        let start = std::time::Instant::now();
        let err = dl.fetch(&url).await.expect_err("ftp must fail");
        let elapsed = start.elapsed();

        // No exponential backoff between attempts: a single attempt fails
        // fast. The attempt count itself is not observable without a
        // socket; the typed error + fast failure are the observables.
        assert!(
            matches!(err, DownloadError::InvalidUrl(_)),
            "builder error must map to InvalidUrl, got: {err:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "no-retry path must fail fast, took {elapsed:?}"
        );
    }

    // FIX-1 #1231 F-12: the decompressed body cap. Both tests build real
    // gzip wire bodies with the existing `async-compression` workspace dep
    // (same helper pattern as the sitemap_parser tests); wreq's
    // `.gzip(true)` client transparently inflates them, so the stream
    // `read_body_capped` accumulates is the DECOMPRESSED payload.

    /// Compress `data` with gzip over tokio (async-compression bufread).
    async fn gzip_compress(data: &[u8]) -> Vec<u8> {
        use async_compression::tokio::bufread::GzipEncoder;
        use tokio::io::{AsyncReadExt, BufReader};

        let mut encoder = GzipEncoder::new(BufReader::new(std::io::Cursor::new(data)));
        let mut out = Vec::new();
        encoder
            .read_to_end(&mut out)
            .await
            .expect("in-memory gzip encode");
        out
    }

    /// F-12 (negative): a "gzip bomb" — 134 bytes on the wire inflating to
    /// 100 KiB — must abort at the configured DECOMPRESSED cap with
    /// `BodyTooLarge`, and because that error classifies PermanentFatal it
    /// must surface after exactly ONE outbound request (no retry).
    #[tokio::test]
    async fn gzip_bomb_is_rejected_at_the_decompressed_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bomb"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(gzip_compress(&[b'a'; 102_400]).await, "application/gzip")
                    .insert_header("content-encoding", "gzip"),
            )
            .mount(&server)
            .await;

        // 64 KiB cap: below the 100 KiB inflated body.
        let bomb_cap: u64 = 64 * 1024;
        let dl = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            10,
            50,
            bomb_cap,
        )
        .expect("test downloader builds");

        let bomb_url: Url = format!("{}/bomb", server.uri())
            .parse()
            .expect("server uri parses");
        match dl.fetch(&bomb_url).await {
            Ok(page) => panic!(
                "100 KiB inflated body must exceed the 64 KiB cap, got {} bytes",
                page.html.len()
            ),
            Err(DownloadError::BodyTooLarge { limit }) => {
                assert_eq!(limit, bomb_cap, "error must carry the configured cap");
            },
            Err(other) => panic!("expected BodyTooLarge, got: {other:?}"),
        }

        let hits = server
            .received_requests()
            .await
            .expect("received requests")
            .len();
        assert_eq!(
            hits, 1,
            "BodyTooLarge is PermanentFatal: exactly one attempt, got {hits}"
        );
    }

    /// F-12 (positive): a gzipped body UNDER the cap is served in full
    /// after transparent decompression — the cap counts decompressed bytes
    /// and leaves the happy path untouched.
    #[tokio::test]
    async fn gzipped_body_under_the_cap_is_served_decompressed() {
        let server = MockServer::start().await;
        let body = b"hello compressed world";
        Mock::given(method("GET"))
            .and(path("/small"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(gzip_compress(body).await, "application/gzip")
                    .insert_header("content-encoding", "gzip"),
            )
            .mount(&server)
            .await;

        let dl = WreqDownloader::new(
            10,
            5,
            Profile::Chrome145,
            None,
            Vec::new(),
            None,
            None,
            3,
            10,
            50,
            crate::domain::downloader_factory::DEFAULT_MAX_PAGE_BYTES,
        )
        .expect("test downloader builds");

        let url: Url = format!("{}/small", server.uri())
            .parse()
            .expect("server uri parses");
        let page = dl
            .fetch(&url)
            .await
            .expect("under-cap gzip body must fetch normally");
        assert_eq!(page.html, "hello compressed world");
    }
}
