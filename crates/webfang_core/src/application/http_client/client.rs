//! HTTP client implementation
//!
//! Wraps `wreq::Client` with retry logic, UA rotation, and WAF detection.

use crate::domain::http_config::HttpClientConfig;
use crate::domain::http_error::{HttpError, HttpResult};
use crate::domain::session_port::{SessionId, SessionPort};
use crate::error::ScraperError;
use crate::infrastructure::http::waf_engine::{InspectionContext, WafInspector};
use crate::infrastructure::user_agent::UserAgentCache;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use rand;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use url::Url;

use wreq::header::{HeaderMap, HeaderName, HeaderValue};
use wreq::Client;

/// Client Hints headers for Chrome 145 (2026 Standard)
/// These headers must match the TLS fingerprint to avoid "Headless Spoofing" detection
const CLIENT_HINTS_SEC_CH_UA: &str =
    "\"Google Chrome\";v=\"145\", \"Chromium\";v=\"145\", \"Not=A?Brand\";v=\"99\"";
const CLIENT_HINTS_SEC_CH_UA_MOBILE: &str = "?0";
const CLIENT_HINTS_SEC_CH_UA_PLATFORM: &str = "\"Linux\"";

/// HTTP client wrapper with configurable retry behavior
///
/// Wraps `wreq::Client` and adds:
/// - Custom headers from config
/// - Status-specific retry logic
/// - User-agent rotation on 403
/// - Exponential backoff on 429 and 5xx
/// - Request timeout configuration
/// - Rate limiting
/// - URL validation
/// - TLS fingerprint rotation
/// - Request timeout configuration
/// - Rate limiting
/// - URL validation
/// - TLS fingerprint rotation
pub struct HttpClient {
    /// Internal wreq client
    client: Client,
    /// Configuration for headers and retry
    config: HttpClientConfig,
    /// Pool of user agents for rotation
    user_agents: Vec<String>,
    /// Rate limiter for requests per minute
    rate_limiter: Option<RateLimiter<NotKeyed, InMemoryState, DefaultClock>>,
    /// Per-domain session health pool (optional)
    session_pool: Option<Arc<dyn SessionPort>>,
}

impl HttpClient {
    /// Create a new HTTP client with the given configuration
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Config` if client creation fails
    pub fn new(config: HttpClientConfig) -> Result<Self, ScraperError> {
        let client = build_wreq_client(&config, None)?;

        let mut user_agents = UserAgentCache::fallback_agents();
        if let Some(ref ua) = config.user_agent {
            user_agents.insert(0, ua.clone());
        }

        // Create rate limiter if configured
        let rate_limiter = if let Some(rpm) = config.rate_limit_rpm {
            if rpm == 0 {
                return Err(ScraperError::Config(
                    "rate_limit_rpm must be greater than 0".into(),
                ));
            }
            let quota =
                Quota::per_minute(NonZeroU32::new(rpm).expect(
                    "invariant: rpm > 0 was already checked above — NonZeroU32 cannot fail",
                ));
            Some(RateLimiter::direct(quota))
        } else {
            None
        };

        debug!("HttpClient created with {} user agents", user_agents.len());
        if let Some(rpm) = config.rate_limit_rpm {
            debug!("Rate limiter configured: {} requests per minute", rpm);
        }

        Ok(Self {
            client,
            config,
            user_agents,
            rate_limiter,
            session_pool: None,
        })
    }

    /// Set the session health pool for per-domain gating.
    #[must_use]
    pub fn with_session_pool(mut self, pool: Arc<dyn SessionPort>) -> Self {
        self.session_pool = Some(pool);
        self
    }
    /// Get a reference to the inner `wreq::Client`.
    ///
    /// Useful when the client needs to be passed to application-layer
    /// functions that expect a raw `&wreq::Client`.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Perform GET request with retry logic
    ///
    /// Handles status codes as follows:
    /// - 200-299: Returns body as String
    /// - 403: Logs + retries once with rotated user-agent
    /// - 429: Exponential backoff respecting Retry-After header
    /// - 500-599: Exponential backoff with automatic retry
    ///
    /// # Errors
    ///
    /// Returns `HttpError` for failed requests or invalid URLs
    pub async fn get(&self, url: &str) -> HttpResult<String> {
        // Validate URL first
        let parsed_url =
            Url::parse(url).map_err(|e| HttpError::Request(format!("Invalid URL: {e}")))?;

        // Ensure URL has http or https scheme
        if !matches!(parsed_url.scheme(), "http" | "https") {
            return Err(HttpError::Request(
                "URL must use http or https scheme".into(),
            ));
        }

        // Session pool gating: acquire before network request
        let (session_id, domain) = if let Some(ref pool) = self.session_pool {
            let domain = crate::application::url_filter::extract_domain(url)
                .unwrap_or_else(|| parsed_url.host_str().unwrap_or("unknown").to_string());
            match pool.acquire(&domain) {
                Some(id) => (Some(id), domain),
                None => return Err(HttpError::DomainBanned(domain)),
            }
        } else {
            (None, String::new())
        };
        // Apply rate limiting if configured
        if let Some(ref limiter) = self.rate_limiter {
            limiter.until_ready().await;
        }

        // Track in-flight requests

        let result = self.get_inner(url).await;

        // Report outcome to session pool
        self.report_outcome(&domain, session_id, &result);

        result
    }

    /// Report request outcome to the session pool for health tracking.
    fn report_outcome(
        &self,
        domain: &str,
        session_id: Option<SessionId>,
        result: &HttpResult<String>,
    ) {
        let (Some(pool), Some(id)) = (self.session_pool.as_ref(), session_id) else {
            return;
        };
        match result {
            Ok(_) => pool.report_success(domain, id),
            Err(e) => {
                let status = match e {
                    HttpError::WafChallenge(_) | HttpError::Forbidden => 403,
                    HttpError::RateLimited(_) => 429,
                    HttpError::Timeout => 504,
                    HttpError::Connection(_) => 503,
                    HttpError::ClientError(404) => return, // don't report 404
                    HttpError::ClientError(code) => *code,
                    HttpError::ServerError(code) => *code,
                    HttpError::Request(_) => return,
                    HttpError::DomainBanned(_) => return,
                };
                pool.report_failure(domain, id, status);
            },
        }
    }

    async fn get_inner(&self, url: &str) -> HttpResult<String> {
        let mut ua_index = 0;
        let max_attempts = self.config.max_retries;

        loop {
            if ua_index >= self.user_agents.len() && ua_index > 0 {
                return Err(HttpError::Forbidden);
            }

            let ua = self
                .user_agents
                .get(ua_index % self.user_agents.len())
                .cloned()
                .unwrap_or_else(|| {
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".into()
                });

            let mut request = self
                .client
                .get(url)
                .header("Accept-Language", &self.config.accept_language)
                .header("Accept", &self.config.accept)
                .header("User-Agent", ua.clone());

            // Use minimal headers for WAF bypass attempts (ua_index >= 4)
            if ua_index < 4 {
                request = request
                    .header("Referer", &self.config.referer)
                    .header("Cache-Control", &self.config.cache_control);
            }

            let response = request.send().await.map_err(|e| {
                if e.is_timeout() {
                    HttpError::Timeout
                } else if e.is_connect() {
                    HttpError::Connection(e.to_string())
                } else {
                    HttpError::Request(e.to_string())
                }
            })?;

            let status = response.status();

            match status.as_u16() {
                200..=299 => {
                    // Build the inspection context BEFORE consuming the body
                    // (headers borrow must end before `.text()`).
                    let ctx = inspection_context(
                        status.as_u16(),
                        response.headers(),
                        self.config.ignore_waf,
                    );

                    let body = response
                        .text()
                        .await
                        .map_err(|e| HttpError::Request(e.to_string()))?;

                    // Context-aware WAF inspection (REQ-WAF-05). A classified
                    // block returns immediately — the old fallback ladder
                    // (~4 requests / ~5s of UA rotation) is gone: with tiered,
                    // evidence-based detection a block is a genuine challenge
                    // that UA rotation cannot bypass, and false positives no
                    // longer reach this branch.
                    let verdict = WafInspector::inspect(&body, &ctx);
                    if verdict.is_blocked {
                        warn!(
                            "WAF/CAPTCHA challenge detected from {} ({} evidences); blocking",
                            url,
                            verdict.evidences.len()
                        );
                        return Err(HttpError::WafChallenge(verdict.evidence_chain()));
                    }

                    return Ok(body);
                },
                403 => {
                    warn!("403 Forbidden from {}", url);
                    if ua_index == 0 {
                        ua_index += 1;
                        continue;
                    }
                    return Err(HttpError::Forbidden);
                },
                429 => {
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1);

                    debug!("429 Rate Limited, retry after {}s", retry_after);

                    let mut attempt = 0;
                    let ua_for_retry = ua.clone();
                    while attempt < max_attempts {
                        attempt += 1;

                        let delay_ms = if retry_after > 0 {
                            retry_after * 1000
                        } else {
                            let exponent = attempt.saturating_sub(1);
                            let delay = self.config.backoff_base_ms * (2_u64.pow(exponent));
                            delay.min(self.config.backoff_max_ms)
                        };

                        debug!("429 retry attempt {} after {}ms", attempt, delay_ms);
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                        let request = self
                            .client
                            .get(url)
                            .header("Accept-Language", &self.config.accept_language)
                            .header("Accept", &self.config.accept)
                            .header("Referer", &self.config.referer)
                            .header("Cache-Control", &self.config.cache_control)
                            .header("User-Agent", &ua_for_retry);

                        match request.send().await {
                            Ok(resp) => {
                                if resp.status().is_success() {
                                    return resp
                                        .text()
                                        .await
                                        .map_err(|e| HttpError::Request(e.to_string()));
                                } else if resp.status().as_u16() == 429
                                    || resp.status().is_server_error()
                                {
                                    continue;
                                } else {
                                    return Err(HttpError::ClientError(resp.status().as_u16()));
                                }
                            },
                            Err(e) => {
                                if e.is_timeout() {
                                    return Err(HttpError::Timeout);
                                }
                                continue;
                            },
                        }
                    }
                    return Err(HttpError::RateLimited(retry_after));
                },
                500..=599 => {
                    debug!("{} from {}", status, url);

                    // Inspect the initial response BEFORE retrying (REQ-WAF-05).
                    // Approved behavior change: a 503 Cloudflare challenge is
                    // classified as a WAF block instead of dying as a generic
                    // ServerError after exhausting retries.
                    let ctx = inspection_context(
                        status.as_u16(),
                        response.headers(),
                        self.config.ignore_waf,
                    );
                    let body = response
                        .text()
                        .await
                        .map_err(|e| HttpError::Request(e.to_string()))?;
                    let verdict = WafInspector::inspect(&body, &ctx);
                    if verdict.is_blocked {
                        warn!(
                            "WAF/CAPTCHA challenge detected from {} on {} ({} evidences); blocking",
                            url,
                            status,
                            verdict.evidences.len()
                        );
                        return Err(HttpError::WafChallenge(verdict.evidence_chain()));
                    }

                    let mut attempt = 0;
                    let ua_for_retry = ua.clone();
                    while attempt < max_attempts {
                        attempt += 1;

                        let exponent = attempt.saturating_sub(1);
                        let delay = self.config.backoff_base_ms * (2_u64.pow(exponent));
                        let delay_ms = delay.min(self.config.backoff_max_ms);

                        debug!("5xx retry attempt {} after {}ms", attempt, delay_ms);
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                        let request = self
                            .client
                            .get(url)
                            .header("Accept-Language", &self.config.accept_language)
                            .header("Accept", &self.config.accept)
                            .header("Referer", &self.config.referer)
                            .header("Cache-Control", &self.config.cache_control)
                            .header("User-Agent", &ua_for_retry);

                        match request.send().await {
                            Ok(resp) => {
                                if resp.status().is_success() {
                                    return resp
                                        .text()
                                        .await
                                        .map_err(|e| HttpError::Request(e.to_string()));
                                } else if resp.status().is_server_error() {
                                    continue;
                                } else {
                                    return Err(HttpError::ClientError(resp.status().as_u16()));
                                }
                            },
                            Err(e) => {
                                if e.is_timeout() {
                                    return Err(HttpError::Timeout);
                                }
                                continue;
                            },
                        }
                    }
                    return Err(HttpError::ServerError(status.as_u16()));
                },
                code if (400..=499).contains(&code) => {
                    return Err(HttpError::ClientError(code));
                },
                code => {
                    return Err(HttpError::ServerError(code));
                },
            }
        }
    }
}

/// Build a WAF [`InspectionContext`] from an HTTP response's status and headers.
///
/// Extracts the content-type for the REQ-WAF-02 gate and clones the headers for
/// control-header evidence (REQ-WAF-03). `ignore_waf` short-circuits inspection
/// to a clean verdict (REQ-WAF-07). The header borrow ends when this returns
/// (the map is cloned), so the caller can still consume the response body.
fn inspection_context(status: u16, headers: &HeaderMap, ignore_waf: bool) -> InspectionContext {
    InspectionContext {
        status: Some(status),
        content_type: headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(String::from),
        headers: headers.clone(),
        ignore_waf,
    }
}

/// Build the inner `wreq::Client` shared by `HttpClient::new` and the
/// bare-client factories.
///
/// Applies the Chrome Client Hints + Sec-Fetch default headers (these MUST
/// match the TLS fingerprint to avoid "Headless Spoofing" detection), the
/// resolved H2/TLS profile, request/connect timeouts, connection-pool tuning,
/// compression, cookie storage, and a bounded redirect policy.
///
/// When `user_agent` is `Some`, it is set as the build-time `User-Agent`;
/// when `None`, no build-time UA is applied (callers such as `HttpClient`
/// set the UA per request instead).
///
/// # Errors
///
/// Returns `ScraperError::Config` if the underlying client fails to build.
fn build_wreq_client(
    config: &HttpClientConfig,
    user_agent: Option<String>,
) -> Result<Client, ScraperError> {
    let pool_size = std::cmp::max(6, num_cpus::get() - 1);

    // Build Client Hints headers for Chrome 145 (2026 Standard)
    // These MUST match the TLS fingerprint to avoid "Headless Spoofing" detection
    let mut headers = HeaderMap::new();
    headers.insert(
        HeaderName::from_static("sec-ch-ua"),
        HeaderValue::from_static(CLIENT_HINTS_SEC_CH_UA),
    );
    headers.insert(
        HeaderName::from_static("sec-ch-ua-mobile"),
        HeaderValue::from_static(CLIENT_HINTS_SEC_CH_UA_MOBILE),
    );
    headers.insert(
        HeaderName::from_static("sec-ch-ua-platform"),
        HeaderValue::from_static(CLIENT_HINTS_SEC_CH_UA_PLATFORM),
    );
    // Additional security headers (Sec-Fetch)
    headers.insert(
        HeaderName::from_static("sec-fetch-dest"),
        HeaderValue::from_static("document"),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-mode"),
        HeaderValue::from_static("navigate"),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-site"),
        HeaderValue::from_static("none"),
    );
    headers.insert(
        HeaderName::from_static("sec-fetch-user"),
        HeaderValue::from_static("?1"),
    );
    headers.insert(
        HeaderName::from_static("upgrade-insecure-requests"),
        HeaderValue::from_static("1"),
    );

    let mut builder = Client::builder()
        .emulation(config.tls_emulation)
        .default_headers(headers)
        .timeout(Duration::from_secs(config.timeout_secs))
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        .pool_max_idle_per_host(pool_size)
        .pool_idle_timeout(Duration::from_secs(60))
        .gzip(true)
        .brotli(true)
        .cookie_store(true)
        .redirect(wreq::redirect::Policy::limited(10));

    if let Some(ua) = user_agent {
        builder = builder.user_agent(ua);
    }

    builder
        .build()
        .map_err(|e| ScraperError::Config(format!("failed to create http client: {e}")))
}

/// Create a bare `wreq::Client` that honors the given domain config.
///
/// This is the config-driven counterpart of `HttpClient::new`: it returns the
/// raw inner client (no retry middleware) for callers that manage requests
/// themselves, while still applying the Chrome Client Hints headers, the
/// resolved H2/TLS profile, request/connect timeouts, and pool tuning from
/// `config`.
///
/// User-agent resolution: `config.user_agent` is used when set; otherwise a
/// random agent is drawn from the fallback pool, preserving the historical
/// rotation behavior of `create_http_client`.
///
/// # Errors
///
/// Returns `ScraperError::Config` if the underlying client fails to build.
pub fn create_http_client_with_config(config: &HttpClientConfig) -> Result<Client, ScraperError> {
    let user_agent = match config.user_agent.clone() {
        Some(ua) => ua,
        None => get_random_user_agent_from_pool(&UserAgentCache::fallback_agents()),
    };

    tracing::debug!("Using user agent: {}", user_agent);

    build_wreq_client(config, Some(user_agent))
}

// Legacy function - simplified, returns wreq::Client directly
/// Create configured HTTP client
///
/// Equivalent to `create_http_client_with_config(&HttpClientConfig::default())`:
/// a Chrome145 client with a random pooled user agent, a 30s request timeout,
/// and a 10s connect timeout, plus the Chrome Client Hints default headers.
/// For more control, use `HttpClient::new()` with `HttpClientConfig`.
pub fn create_http_client() -> Result<Client, ScraperError> {
    create_http_client_with_config(&HttpClientConfig::default())
}

/// Get random user agent from pool (legacy function)
pub fn get_random_user_agent_from_pool(pool: &[String]) -> String {
    use rand::Rng;
    let mut rng = rand::rng();
    let index = rng.random_range(0..pool.len());
    pool[index].clone()
}

// ============================================================================
// HttpClientPort implementation for HttpClient
// ============================================================================

impl crate::domain::http_port::HttpClientPort for HttpClient {
    fn get(
        &self,
        url: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::domain::http_error::HttpResult<
                        crate::domain::http_port::HttpResponse,
                    >,
                > + Send
                + '_,
        >,
    > {
        let url = url.to_owned();
        Box::pin(async move {
            // Use the inner wreq client for the raw request, then build
            // a full HttpResponse (status + body + headers).
            let resp = self.client.get(url.as_str()).send().await.map_err(|e| {
                if e.is_timeout() {
                    crate::domain::http_error::HttpError::Timeout
                } else if e.is_connect() {
                    crate::domain::http_error::HttpError::Connection(e.to_string())
                } else {
                    crate::domain::http_error::HttpError::Request(e.to_string())
                }
            })?;

            let status = resp.status().as_u16();
            let mut headers = std::collections::HashMap::new();
            for (key, value) in resp.headers() {
                if let Ok(v) = value.to_str() {
                    headers.insert(key.as_str().to_lowercase(), v.to_string());
                }
            }
            let body = resp
                .text()
                .await
                .map_err(|e| crate::domain::http_error::HttpError::Request(e.to_string()))?;

            Ok(crate::domain::http_port::HttpResponse {
                status,
                body,
                headers,
            })
        })
    }
}

#[cfg(test)]
#[cfg(not(miri))] // all tests create wreq::Client with boring-sys2 FFI (unsupported by Miri)
mod tests {
    use super::*;
    use crate::domain::http_config::HttpClientConfig;

    #[test]
    fn test_http_client_creation_default() {
        let config = HttpClientConfig::default();
        let result = HttpClient::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_client_has_user_agents() {
        let config = HttpClientConfig::default();

        assert!(
            config.max_retries > 0,
            "HttpClientConfig should have positive max_retries default"
        );
        assert!(
            config.backoff_base_ms > 0,
            "HttpClientConfig should have positive backoff_base_ms default"
        );

        let _client = HttpClient::new(config).unwrap();
    }

    #[tokio::test]
    async fn test_http_client_get_invalid_url() {
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get("not-a-valid-url").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_url_validation_invalid_scheme() {
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get("ftp://example.com").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::Request(_)));
    }

    /// Construction smoke test: a non-default `tls_emulation` preset builds a
    /// client successfully. `wreq::Client` does not expose its applied profile,
    /// so the meaningful profile-mapping assertions live at the config level
    /// (`http_config::profile_from_name` and `scrape_flow::build_http_client_config`).
    #[test]
    fn test_http_client_with_custom_tls_emulation() {
        let config = HttpClientConfig {
            tls_emulation: wreq_util::Profile::Chrome131,
            ..Default::default()
        };
        let result = HttpClient::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_client_with_rate_limiting() {
        let config = HttpClientConfig {
            rate_limit_rpm: Some(60),
            ..Default::default()
        };
        let result = HttpClient::new(config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore = "requires network - run with cargo test --ignored"]
    async fn test_http_client_get_example_com() {
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get("https://example.com").await;
        assert!(result.is_ok());

        let body = result.unwrap();
        assert!(!body.is_empty());
    }
}

#[cfg(test)]
#[cfg(not(miri))] // all tests create wreq::Client with boring-sys2 FFI (unsupported by Miri)
mod wiremock_tests {
    use super::*;
    use crate::domain::http_config::HttpClientConfig;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_403_returns_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 1,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::Forbidden));
    }

    #[tokio::test]
    async fn test_429_returns_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "1"))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 1,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let start = std::time::Instant::now();
        let result = client.get(&mock_server.uri()).await;
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::RateLimited(_)));
        assert!(elapsed.as_millis() >= 10);
    }

    #[tokio::test]
    async fn test_500_returns_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 1,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::ServerError(500)));
    }

    #[tokio::test]
    async fn test_500_exhausts_retries() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 2,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::ServerError(500)));
    }

    #[tokio::test]
    async fn test_404_returns_client_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/notfound"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&format!("{}/notfound", mock_server.uri())).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::ClientError(404)));
    }

    #[tokio::test]
    async fn test_200_returns_body() {
        let mock_server = MockServer::start().await;

        let expected_body = "<html><body>Hello World</body></html>";
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(expected_body))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected_body);
    }

    /// `user_agent: Some(..)` in `HttpClientConfig` must reach the wire as a
    /// `user-agent` header.
    ///
    /// `wreq::Client` does not expose its applied profile or default headers,
    /// so we verify at the wire: the mock only answers 200 when the request
    /// carries the exact `user-agent` we configured, and `.expect(1)` plus
    /// `server.verify()` prove the header matched exactly once. Real network
    /// I/O to localhost, hence not(miri).
    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_custom_user_agent_reaches_wire() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .and(header("user-agent", "custom-ua-test"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .expect(1)
            .mount(&server)
            .await;

        let config = HttpClientConfig {
            user_agent: Some("custom-ua-test".to_string()),
            ..Default::default()
        };
        let client = create_http_client_with_config(&config).unwrap();

        let response = client.get(server.uri()).send().await.unwrap();
        assert_eq!(response.status().as_u16(), 200);

        server.verify().await;
    }
}

#[cfg(test)]
#[cfg(not(miri))] // all tests create wreq::Client with boring-sys2 FFI (unsupported by Miri)
mod waf_detection_tests {
    use super::*;
    use crate::domain::http_config::HttpClientConfig;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_200_cloudflare_challenge_returns_waf_error() {
        let mock_server = MockServer::start().await;

        let challenge_body = r#"<html><head><title>Just a moment...</title></head>
        <body><div id="challenge-running">Checking your browser...</div></body></html>"#;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(challenge_body))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 1,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::WafChallenge(_)));
    }

    #[tokio::test]
    async fn test_200_recaptcha_challenge_returns_waf_error() {
        let mock_server = MockServer::start().await;

        let challenge_body =
            r#"<html><body><div class="g-recaptcha" data-sitekey="abc"></div></body></html>"#;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(challenge_body))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 1,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::WafChallenge(_)));
    }

    #[tokio::test]
    async fn test_200_normal_page_returns_body() {
        let mock_server = MockServer::start().await;

        let normal_body =
            "<html><body><article><h1>Real Content</h1><p>Normal page.</p></article></body></html>";

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(normal_body))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), normal_body);
    }

    // ========================================================================
    // TASK-10 — context-aware inspect() in the 2xx + 5xx branches (REQ-WAF-05/07)
    // ========================================================================

    /// Approved behavior change: a 503 Cloudflare challenge is classified as a
    /// WAF block (WafChallenge) instead of dying as a generic ServerError(503)
    /// after exhausting retries. `.expect(1)` proves the block returns
    /// immediately — no retry loop, no wasted requests.
    #[tokio::test]
    async fn test_503_cloudflare_challenge_returns_waf_error_not_server_error() {
        let mock_server = MockServer::start().await;

        let challenge_body = r#"<html><head><title>Just a moment...</title></head>
        <body><div id="challenge-running">Checking your browser...</div></body></html>"#;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .set_body_string(challenge_body),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 3,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        let err = result.expect_err("503 challenge must be a WAF block, not Ok");
        assert!(
            matches!(err, HttpError::WafChallenge(_)),
            "expected WafChallenge, got {err:?}"
        );
        mock_server.verify().await;
    }

    /// A genuine 5xx error with no WAF signature still surfaces as ServerError
    /// (triangulation: the 5xx inspection only diverts actual challenges).
    ///
    /// RES-01: the response carries a `cf-ray` header — present on EVERY
    /// Cloudflare edge response, including genuine transient origin failures —
    /// which must NOT turn a plain 503 into a WAF block.
    #[tokio::test]
    async fn test_503_plain_error_still_returns_server_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("content-type", "text/plain")
                    .insert_header("cf-ray", "abc123-IAD")
                    .set_body_string("service temporarily unavailable"),
            )
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 1,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(
            matches!(result.unwrap_err(), HttpError::ServerError(503)),
            "plain 503 with a cf-ray trace header must stay a ServerError"
        );
    }

    /// RES-01 (real-world case): a genuine transient 503 behind Cloudflare — a
    /// generic HTML error body plus the ubiquitous `cf-ray` edge header — must
    /// surface as ServerError after retries, NOT as a WAF block. `cf-ray` rides
    /// on 100% of Cloudflare traffic and carries zero challenge evidence, so it
    /// must not correlate with the 503 to fabricate a false positive.
    #[tokio::test]
    async fn test_503_cf_ray_generic_error_still_returns_server_error() {
        let mock_server = MockServer::start().await;

        let error_body = "<html><body><h1>503 Service Unavailable</h1>\
            <p>The server is temporarily unable to service your request.</p></body></html>";

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .insert_header("cf-ray", "abc123-IAD")
                    .set_body_string(error_body),
            )
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 2,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        let err = result.expect_err("generic 503 with cf-ray must not be Ok");
        assert!(
            matches!(err, HttpError::ServerError(503)),
            "expected ServerError(503), got {err:?}"
        );
    }

    /// RES-01 guard: purging the ubiquitous trace headers must NOT weaken the
    /// correlated-mitigation case. A 503 carrying `cf-mitigated` (an active
    /// Cloudflare mitigation signal, retained in the registry) is still a WAF
    /// block — the Fingerprint header correlates with the 503 status. `.expect(1)`
    /// proves the block returns immediately with no retry.
    #[tokio::test]
    async fn test_503_cf_mitigated_still_returns_waf_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .insert_header("cf-mitigated", "challenge")
                    .set_body_string("<html><body>temporarily unavailable</body></html>"),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 3,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        let err = result.expect_err("cf-mitigated 503 must be a WAF block, not Ok");
        assert!(
            matches!(err, HttpError::WafChallenge(_)),
            "expected WafChallenge, got {err:?}"
        );
        mock_server.verify().await;
    }

    /// FIX B (body-vs-header granularity): a genuine transient 503 whose BODY
    /// merely mentions a vendor (bare "cloudflare", T2 Body) is ubiquitous
    /// diagnostic noise — NOT a WAF block. It must surface as `ServerError(503)`
    /// after retries, not as an instant `WafChallenge`. A control HEADER such as
    /// `cf-mitigated` would still block (see
    /// `test_503_cf_mitigated_still_returns_waf_error`).
    #[tokio::test]
    async fn test_503_bare_body_vendor_mention_retries_then_server_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(503)
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .set_body_string("<html><body>served by cloudflare</body></html>"),
            )
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 2,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(
            matches!(result.unwrap_err(), HttpError::ServerError(503)),
            "bare body vendor mention at 503 must stay a ServerError (retries), not a WAF block"
        );
    }

    /// REQ-WAF-02/04/05 at the client boundary: a 200 `application/json` body
    /// carrying `akamai_hash` (the tls.peet.ws false positive from issue #346)
    /// passes — the content-type gate skips scanning JSON entirely.
    #[tokio::test]
    async fn test_200_json_akamai_hash_passes() {
        let mock_server = MockServer::start().await;

        let json_body = r#"{"tls":{"peetprint_hash":"abc123","akamai_hash":"def456"}}"#;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "application/json")
                    .set_body_string(json_body),
            )
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_ok(), "JSON akamai_hash must pass, got {result:?}");
        assert_eq!(result.unwrap(), json_body);
    }

    /// A classified 200 challenge returns immediately: `.expect(1)` proves the
    /// old fallback ladder (~4 requests / ~5s of UA rotation) is gone.
    #[tokio::test]
    async fn test_200_classified_block_skips_fallback_ladder() {
        let mock_server = MockServer::start().await;

        let challenge_body =
            r#"<html><body><div class="g-recaptcha" data-sitekey="abc"></div></body></html>"#;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html; charset=utf-8")
                    .set_body_string(challenge_body),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(matches!(result.unwrap_err(), HttpError::WafChallenge(_)));
        mock_server.verify().await;
    }
}

#[cfg(test)]
#[cfg(not(miri))]
mod session_pool_tests {
    use super::*;
    use crate::domain::http_config::HttpClientConfig;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Mock SessionPort for testing.
    struct MockSessionPort {
        /// Controls what acquire returns: Some(SessionId(0)) or None (banned).
        banned: std::sync::atomic::AtomicBool,
        /// Counts calls to report_success and report_failure.
        success_count: AtomicUsize,
        failure_count: AtomicUsize,
        last_failure_status: std::sync::atomic::AtomicU16,
    }

    impl MockSessionPort {
        fn new(banned: bool) -> Arc<Self> {
            Arc::new(Self {
                banned: std::sync::atomic::AtomicBool::new(banned),
                success_count: AtomicUsize::new(0),
                failure_count: AtomicUsize::new(0),
                last_failure_status: std::sync::atomic::AtomicU16::new(0),
            })
        }
    }

    impl SessionPort for MockSessionPort {
        fn acquire(&self, _domain: &str) -> Option<SessionId> {
            if self.banned.load(Ordering::SeqCst) {
                None
            } else {
                Some(SessionId(0))
            }
        }

        fn report_success(&self, _domain: &str, _session: SessionId) {
            self.success_count.fetch_add(1, Ordering::SeqCst);
        }

        fn report_failure(&self, _domain: &str, _session: SessionId, status: u16) {
            self.failure_count.fetch_add(1, Ordering::SeqCst);
            self.last_failure_status.store(status, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn test_acquire_banned_returns_domain_banned_error() {
        let pool = MockSessionPort::new(true);
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config)
            .unwrap()
            .with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

        let result = client.get("https://example.com/page").await;

        assert!(result.is_err());
        match result.unwrap_err() {
            HttpError::DomainBanned(domain) => assert_eq!(domain, "example.com"),
            other => panic!("expected DomainBanned, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_acquire_healthy_proceeds_with_request() {
        let pool = MockSessionPort::new(false);
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config)
            .unwrap()
            .with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

        // Use a non-existent URL — we just want to verify it doesn't return DomainBanned
        let result = client.get("https://httpbin.org/get").await;
        // Should NOT be DomainBanned — could be any other error (network, etc.)
        if let Err(HttpError::DomainBanned(_)) = &result {
            panic!("should not be DomainBanned for healthy pool")
        }
    }

    #[tokio::test]
    async fn test_report_outcome_success_calls_report_success() {
        let pool = MockSessionPort::new(false);
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config)
            .unwrap()
            .with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&mock_server)
            .await;

        let _ = client.get(&mock_server.uri()).await;

        assert_eq!(pool.success_count.load(Ordering::SeqCst), 1);
        assert_eq!(pool.failure_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_report_outcome_403_calls_report_failure() {
        let pool = MockSessionPort::new(false);
        let config = HttpClientConfig {
            max_retries: 1,
            ..Default::default()
        };
        let client = HttpClient::new(config)
            .unwrap()
            .with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let _ = client.get(&mock_server.uri()).await;

        assert_eq!(pool.failure_count.load(Ordering::SeqCst), 1);
        assert_eq!(pool.last_failure_status.load(Ordering::SeqCst), 403);
    }

    #[tokio::test]
    async fn test_report_outcome_404_no_report_failure() {
        let pool = MockSessionPort::new(false);
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config)
            .unwrap()
            .with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/notfound"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let _ = client.get(&format!("{}/notfound", mock_server.uri())).await;

        assert_eq!(pool.success_count.load(Ordering::SeqCst), 0);
        assert_eq!(pool.failure_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_builder_sets_session_pool() {
        let pool = MockSessionPort::new(false);
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config)
            .unwrap()
            .with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

        assert!(client.session_pool.is_some());
    }

    #[test]
    fn test_builder_without_session_pool() {
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        assert!(client.session_pool.is_none());
    }
}
