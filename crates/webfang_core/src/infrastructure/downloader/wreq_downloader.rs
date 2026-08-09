//! Wreq-based downloader implementation.
//!
//! Wraps a shared `wreq::Client` behind `Arc` for connection pooling.
//! Extracts cookies from responses and returns [`FetchedPage`] with HTML + cookies.
//!
//! Following **own-arc-shared**: Uses `Arc<Client>` for thread-safe shared ownership
//! of the connection pool. The client is created once and shared across all requests.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use tracing::{debug, instrument, warn};
use url::Url;
use wreq::Client;
use wreq_util::Profile;

use super::{Cookie, DownloadError, Downloader, FetchedPage};
use crate::infrastructure::user_agent::UserAgentCache;

/// Estimated memory cost of a wreq client instance in bytes.
///
/// This accounts for the connection pool, TLS session cache, and internal buffers.
/// Value is approximate — real usage varies by pool size and active connections.
const WREQ_MEMORY_COST: usize = 1_024 * 1_024; // ~1 MB

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
/// let downloader = WreqDownloader::new(30, 10, wreq_util::Profile::Chrome145, None, 3, 1000, 10000).unwrap();
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
    /// Base delay for exponential backoff — reserved for future jitter implementation.
    #[allow(dead_code)]
    backoff_base_ms: u64,
    backoff_max_ms: u64,
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
    ///
    /// # Errors
    ///
    /// Returns [`DownloadError::Internal`] if the wreq client cannot be built.
    pub fn new(
        timeout_secs: u64,
        connect_timeout_secs: u64,
        tls_emulation: Profile,
        user_agent: Option<String>,
        max_retries: u32,
        backoff_base_ms: u64,
        backoff_max_ms: u64,
    ) -> Result<Self, DownloadError> {
        let pool_size = std::cmp::max(6, num_cpus::get() - 1);

        // `.emulation(profile)` installs the profile-default headers (including
        // a browser UA) via `default_headers`; a pinned UA must be set AFTER
        // it so `HeaderMap::insert` replaces the profile value (#503).
        let builder = Client::builder().emulation(tls_emulation);
        let builder = match user_agent.as_deref() {
            Some(ua) => builder.user_agent(ua),
            None => builder,
        };
        let client = builder
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(connect_timeout_secs))
            .pool_max_idle_per_host(pool_size)
            .pool_idle_timeout(Duration::from_secs(60))
            .gzip(true)
            .brotli(true)
            .cookie_store(true)
            .redirect(wreq::redirect::Policy::limited(10))
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
        })
    }

    /// Create a WreqDownloader from an existing `wreq::Client`.
    ///
    /// Useful when you need custom client configuration beyond the defaults.
    pub fn from_client(client: Client, timeout_secs: u64, _connect_timeout_secs: u64) -> Self {
        Self {
            client: Arc::new(client),
            timeout_secs,
            pinned_ua: None,
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
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
                DownloadError::Network(Box::new(e))
            }
        })
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

        let mut response = self.send_request(url, None).await?;
        let mut status = response.status().as_u16();

        // 403: one implicit retry with rotated User-Agent (mirrors HttpClient).
        // Pinned UA (#503): the operator asked to be identified exactly as
        // configured ("--user-agent QA-Bot/9.9" means QA-Bot or fail), so pool
        // rotation is skipped and the 403 falls through to normal error handling.
        if status == 403 && self.pinned_ua.is_none() {
            warn!("403 Forbidden from {url} — retrying with rotated User-Agent");
            let agents = UserAgentCache::fallback_agents();
            let rotated_ua = agents.get(1).map(String::as_str);
            response = self.send_request(url, rotated_ua).await?;
            status = response.status().as_u16();
        }

        // 429: backoff retry up to max_retries times, honouring Retry-After.
        if status == 429 {
            let delay_ms = parse_retry_after_ms(&response, self.backoff_max_ms);
            warn!(
                delay_ms = delay_ms,
                "429 Rate Limited from {url} — retrying after {delay_ms}ms"
            );
            let mut succeeded = false;
            for _ in 0..self.max_retries {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                response = self.send_request(url, None).await?;
                status = response.status().as_u16();
                if response.status().is_success() {
                    succeeded = true;
                    break;
                }
            }
            if !succeeded {
                return Err(DownloadError::Http {
                    status: 429,
                    message: "HTTP 429".to_string(),
                });
            }
        }

        // Remaining non-2xx → terminal error.
        if !response.status().is_success() {
            let message = format!("HTTP {status}");
            return Err(DownloadError::Http { status, message });
        }

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

        let html = response
            .text()
            .await
            .map_err(|e| DownloadError::Network(Box::new(e)))?;

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

        let downloader =
            WreqDownloader::new(10, 5, Profile::Chrome145, None, 3, 1000, 10000).unwrap();
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
        let downloader =
            WreqDownloader::new(30, 10, Profile::Chrome145, None, 3, 1000, 10000).unwrap();
        assert!(!downloader.supports_interactions());
        assert_eq!(downloader.memory_cost(), WREQ_MEMORY_COST);
    }

    #[test]
    fn test_wreq_downloader_honors_tls_profile() {
        // The constructor must accept every catalog profile and build a client
        // with it (triangulation: the parameter reaches the builder instead of
        // a hardcoded default).
        for profile in [Profile::Chrome145, Profile::Chrome131, Profile::Firefox135] {
            let downloader = WreqDownloader::new(30, 10, profile, None, 3, 1000, 10000)
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

        let downloader =
            WreqDownloader::new(10, 5, Profile::Chrome145, None, 3, 1000, 10000).unwrap();
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

        let downloader =
            WreqDownloader::new(10, 5, Profile::Chrome145, None, 3, 1000, 10000).unwrap();
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

        let downloader =
            WreqDownloader::new(10, 5, Profile::Chrome145, None, 3, 1000, 10000).unwrap();
        let url: Url = format!("{}/redirect", mock_server.uri()).parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_ok());

        let page = result.unwrap();
        assert!(page.url.as_str().contains("/target"));
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
            3,
            1000,
            10000,
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
            3,
            1000,
            10000,
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

        let downloader =
            WreqDownloader::new(10, 5, Profile::Chrome145, None, 3, 1000, 10000).unwrap();
        let url: Url = mock_server.uri().parse().unwrap();

        let page = downloader
            .fetch(&url)
            .await
            .expect("rotated retry succeeds for unpinned downloads");
        assert_eq!(page.status, 200);
        assert_eq!(page.html, "<html>rotated</html>");
    }
}
