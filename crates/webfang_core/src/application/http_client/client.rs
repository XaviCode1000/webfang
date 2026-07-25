//! HTTP client implementation
//!
//! Wraps `wreq::Client` with retry logic, UA rotation, and WAF detection.

use crate::domain::http_config::HttpClientConfig;
use crate::domain::http_error::{HttpError, HttpResult};
use crate::domain::session_port::SessionPort;
use crate::error::ScraperError;
use crate::infrastructure::http::waf_engine::WafInspector;
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

#[cfg(feature = "otel-metrics")]
use std::time::Instant;

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    in_flight_dec, in_flight_inc, HTTP_DURATION, HTTP_ERRORS,
};
use wreq::header::{HeaderMap, HeaderName, HeaderValue};
use wreq::Client;
use wreq_util::Emulation;

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
    /// Session health pool for domain-level ban tracking (optional)
    session_pool: Option<Arc<dyn SessionPort>>,
}

impl HttpClient {
    /// Create a new HTTP client with the given configuration
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Config` if client creation fails
    pub fn new(config: HttpClientConfig) -> Result<Self, ScraperError> {
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

        // Resolve H2/TLS profile from name — overrides tls_emulation if h2_profile is set
        let profile = HttpClientConfig::resolve_profile(&config.h2_profile);

        let builder = Client::builder()
            .emulation(profile)
            .default_headers(headers)
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
            .pool_max_idle_per_host(pool_size)
            .pool_idle_timeout(Duration::from_secs(60))
            .gzip(true)
            .brotli(true)
            .cookie_store(true)
            .redirect(wreq::redirect::Policy::limited(10));

        let client = builder
            .build()
            .map_err(|e| ScraperError::Config(format!("failed to create http client: {e}")))?;

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

    /// Create a builder for constructing an `HttpClient` with optional session pool.
    ///
    /// The builder pattern allows attaching a `SessionPort` for domain-level
    /// ban tracking while keeping `new()` backward compatible (pool = None).
    pub fn builder(config: HttpClientConfig) -> HttpClientBuilder {
        HttpClientBuilder {
            config,
            session_pool: None,
        }
    }

    /// Get a reference to the inner `wreq::Client`.
    ///
    /// Useful when the client needs to be passed to application-layer
    /// functions that expect a raw `&wreq::Client`.
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Report request outcome to the session pool for domain health tracking.
    ///
    /// Maps `HttpError` variants to penalty status codes:
    /// - WafChallenge/Forbidden → 403 (HIGH penalty)
    /// - RateLimited → 429 (HIGH penalty)
    /// - Timeout → 504 (MEDIUM penalty)
    /// - Connection → 503 (LOW penalty)
    /// - ClientError(404) → no report (healthy server)
    /// - Ok → report_success
    fn report_outcome(
        &self,
        domain: &str,
        session: crate::domain::session_port::SessionId,
        result: &HttpResult<String>,
    ) {
        if let Some(ref pool) = self.session_pool {
            match result {
                Ok(_) => pool.report_success(domain, session),
                Err(e) => {
                    let status = match e {
                        HttpError::WafChallenge(_) | HttpError::Forbidden => 403,
                        HttpError::RateLimited(_) => 429,
                        HttpError::Timeout => 504,
                        HttpError::Connection(_) => 503,
                        // No penalty: ClientError (healthy server), ServerError (transient,
                        // not domain-level blocking), Request (client-side), DomainBanned (already handled).
                        // ServerError 5xx are transient — banning would be too aggressive.
                        HttpError::ClientError(_)
                        | HttpError::ServerError(_)
                        | HttpError::Request(_)
                        | HttpError::DomainBanned(_) => return,
                    };
                    pool.report_failure(domain, session, status);
                },
            }
        }
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

        // Session pool check (sync, fast) — BEFORE rate limiter to save tokens
        let domain = parsed_url.host_str().map(String::from);
        let session_id = if let (Some(ref pool), Some(ref domain)) = (&self.session_pool, &domain) {
            match pool.acquire(domain) {
                Some(id) => Some(id),
                None => return Err(HttpError::DomainBanned(domain.clone())),
            }
        } else {
            None
        };

        // Apply rate limiting if configured
        if let Some(ref limiter) = self.rate_limiter {
            limiter.until_ready().await;
        }

        // Track in-flight requests
        #[cfg(feature = "otel-metrics")]
        in_flight_inc();

        let result = self.get_inner(url).await;

        // Report outcome to session pool
        if let (Some(ref domain), Some(sid)) = (&domain, session_id) {
            self.report_outcome(domain, sid, &result);
        }

        #[cfg(feature = "otel-metrics")]
        {
            in_flight_dec();
            match &result {
                Ok(_) => {},
                Err(e) => {
                    let error_type = match e {
                        HttpError::Timeout => "timeout",
                        HttpError::Forbidden => "forbidden",
                        HttpError::WafChallenge(_) => "waf_challenge",
                        HttpError::RateLimited(_) => "rate_limited",
                        HttpError::ClientError(_) => "client_error",
                        HttpError::ServerError(_) => "server_error",
                        HttpError::Connection(_) => "connection",
                        HttpError::Request(_) => "request",
                        HttpError::DomainBanned(_) => "domain_banned",
                    };
                    HTTP_ERRORS.add(1, &[opentelemetry::KeyValue::new("error_type", error_type)]);
                },
            }
        }

        result
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

            #[cfg(feature = "otel-metrics")]
            let request_start = Instant::now();

            let response = request.send().await.map_err(|e| {
                #[cfg(feature = "otel-metrics")]
                {
                    let elapsed = request_start.elapsed().as_secs_f64();
                    HTTP_DURATION.record(elapsed, &[opentelemetry::KeyValue::new("method", "GET")]);
                }
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
                    #[cfg(feature = "otel-metrics")]
                    {
                        let elapsed = request_start.elapsed().as_secs_f64();
                        HTTP_DURATION
                            .record(elapsed, &[opentelemetry::KeyValue::new("method", "GET")]);
                    }

                    let body = response
                        .text()
                        .await
                        .map_err(|e| HttpError::Request(e.to_string()))?;

                    if let Some(provider) = WafInspector::detect_body(&body) {
                        warn!(
                            "WAF challenge detected from {} ({}), attempting fallback strategies",
                            url, provider
                        );

                        // Fallback strategy 1: Rotate user agent (up to 3 attempts)
                        if ua_index < 3 {
                            ua_index += 1;
                            // Add small delay to avoid rapid retries
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            continue;
                        }

                        // Fallback strategy 2: Try with minimal headers (remove referer/cache-control)
                        if ua_index == 3 {
                            ua_index += 1;
                            warn!("Trying minimal headers for WAF bypass");
                            continue; // Will use different headers below
                        }

                        // Fallback strategy 3: Add random delay (1-3 seconds)
                        if ua_index == 4 {
                            use rand::Rng;
                            ua_index += 1;
                            let delay_ms = 1000 + (rand::rng().random::<u64>() % 2000);
                            warn!("Adding random delay {}ms for WAF bypass", delay_ms);
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            continue;
                        }

                        return Err(HttpError::WafChallenge(provider.to_string()));
                    }

                    return Ok(body);
                },
                403 => {
                    #[cfg(feature = "otel-metrics")]
                    {
                        let elapsed = request_start.elapsed().as_secs_f64();
                        HTTP_DURATION
                            .record(elapsed, &[opentelemetry::KeyValue::new("method", "GET")]);
                    }

                    warn!("403 Forbidden from {}", url);
                    if ua_index == 0 {
                        ua_index += 1;
                        continue;
                    }
                    return Err(HttpError::Forbidden);
                },
                429 => {
                    #[cfg(feature = "otel-metrics")]
                    {
                        let elapsed = request_start.elapsed().as_secs_f64();
                        HTTP_DURATION
                            .record(elapsed, &[opentelemetry::KeyValue::new("method", "GET")]);
                    }

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
                    #[cfg(feature = "otel-metrics")]
                    {
                        let elapsed = request_start.elapsed().as_secs_f64();
                        HTTP_DURATION
                            .record(elapsed, &[opentelemetry::KeyValue::new("method", "GET")]);
                    }

                    debug!("{} from {}", status, url);

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
                    #[cfg(feature = "otel-metrics")]
                    {
                        let elapsed = request_start.elapsed().as_secs_f64();
                        HTTP_DURATION
                            .record(elapsed, &[opentelemetry::KeyValue::new("method", "GET")]);
                    }
                    return Err(HttpError::ClientError(code));
                },
                code => {
                    #[cfg(feature = "otel-metrics")]
                    {
                        let elapsed = request_start.elapsed().as_secs_f64();
                        HTTP_DURATION
                            .record(elapsed, &[opentelemetry::KeyValue::new("method", "GET")]);
                    }
                    return Err(HttpError::ServerError(code));
                },
            }
        }
    }
}

/// Builder for constructing `HttpClient` with optional session pool.
///
/// Use `HttpClient::builder(config)` to create a builder, then chain
/// `.session_pool(pool)` before calling `.build()`.
pub struct HttpClientBuilder {
    config: HttpClientConfig,
    session_pool: Option<Arc<dyn SessionPort>>,
}

impl HttpClientBuilder {
    /// Attach a session health pool for domain-level ban tracking.
    #[must_use]
    pub fn session_pool(mut self, pool: Arc<dyn SessionPort>) -> Self {
        self.session_pool = Some(pool);
        self
    }

    /// Build the `HttpClient`.
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Config` if client creation fails.
    pub fn build(self) -> Result<HttpClient, ScraperError> {
        let mut client = HttpClient::new(self.config)?;
        client.session_pool = self.session_pool;
        Ok(client)
    }
}

// Legacy function - simplified, returns wreq::Client directly
/// Create configured HTTP client
///
/// This function creates a client with basic configuration.
/// For more control, use `HttpClient::new()` with `HttpClientConfig`.
pub fn create_http_client() -> Result<Client, ScraperError> {
    let agents = UserAgentCache::fallback_agents();
    let user_agent = get_random_user_agent_from_pool(&agents);

    tracing::debug!("Using user agent: {}", user_agent);

    let client = Client::builder()
        .emulation(Emulation::Chrome145)
        .user_agent(user_agent)
        .timeout(Duration::from_secs(30))
        .gzip(true)
        .brotli(true)
        .cookie_store(true)
        .redirect(wreq::redirect::Policy::limited(10))
        .build()
        .map_err(|e| ScraperError::Config(format!("failed to create http client: {e}")))?;

    Ok(client)
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
    use wiremock::matchers::{method, path};
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
}

// ============================================================================
// SessionPort integration tests — verify full get() flow with MockSessionPort
// ============================================================================

#[cfg(test)]
#[cfg(not(miri))]
mod session_port_integration_tests {
    use super::*;
    use crate::domain::http_config::HttpClientConfig;
    use crate::domain::session_port::{SessionId, SessionPort};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Test double that records all SessionPort calls for assertions.
    struct MockSessionPort {
        acquire_result: Option<SessionId>,
        success_calls: std::sync::Mutex<Vec<(String, SessionId)>>,
        failure_calls: std::sync::Mutex<Vec<(String, SessionId, u16)>>,
    }

    impl MockSessionPort {
        fn healthy() -> Self {
            Self {
                acquire_result: Some(SessionId(0)),
                success_calls: std::sync::Mutex::new(Vec::new()),
                failure_calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn banned() -> Self {
            Self {
                acquire_result: None,
                success_calls: std::sync::Mutex::new(Vec::new()),
                failure_calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn success_count(&self) -> usize {
            self.success_calls.lock().unwrap().len()
        }

        fn failure_count(&self) -> usize {
            self.failure_calls.lock().unwrap().len()
        }

        fn last_failure_status(&self) -> Option<u16> {
            self.failure_calls
                .lock()
                .unwrap()
                .last()
                .map(|(_, _, status)| *status)
        }
    }

    impl SessionPort for MockSessionPort {
        fn acquire(&self, _domain: &str) -> Option<SessionId> {
            self.acquire_result
        }

        fn report_success(&self, domain: &str, session: SessionId) {
            self.success_calls
                .lock()
                .unwrap()
                .push((domain.to_string(), session));
        }

        fn report_failure(&self, domain: &str, session: SessionId, status: u16) {
            self.failure_calls
                .lock()
                .unwrap()
                .push((domain.to_string(), session, status));
        }
    }

    /// Build an HttpClient with a MockSessionPort attached.
    fn client_with_pool(pool: Arc<dyn SessionPort>) -> HttpClient {
        HttpClient::builder(HttpClientConfig::default())
            .session_pool(pool)
            .build()
            .unwrap()
    }

    // ── Test: acquire() returns None → DomainBanned ──

    #[tokio::test]
    async fn test_acquire_banned_returns_domain_banned() {
        let pool = Arc::new(MockSessionPort::banned());
        let client = client_with_pool(pool);

        let result = client.get("https://example.com").await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), HttpError::DomainBanned(d) if d == "example.com"),
            "Expected DomainBanned for example.com"
        );
    }

    // ── Test: acquire() returns Some → request proceeds → report_success ──

    #[tokio::test]
    async fn test_acquire_healthy_proceeds_and_reports_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<p>OK</p>"))
            .mount(&mock_server)
            .await;

        let pool = Arc::new(MockSessionPort::healthy());
        let client = client_with_pool(pool.clone());

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_ok(), "Request should succeed");
        assert_eq!(pool.success_count(), 1, "report_success should be called once");
        assert_eq!(pool.failure_count(), 0, "report_failure should not be called");
    }

    // ── Test: 403 response → report_failure(403) ──

    #[tokio::test]
    async fn test_report_outcome_maps_403_to_failure() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let pool = Arc::new(MockSessionPort::healthy());
        let client = HttpClient::builder(HttpClientConfig {
            max_retries: 1,
            ..Default::default()
        })
        .session_pool(pool.clone())
        .build()
        .unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_err());
        assert_eq!(pool.failure_count(), 1, "report_failure should be called once");
        assert_eq!(
            pool.last_failure_status(),
            Some(403),
            "Failure status should be 403 (HIGH penalty)"
        );
    }

    // ── Test: 429 response → report_failure(429) ──

    #[tokio::test]
    async fn test_report_outcome_maps_429_to_failure() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(429).insert_header("retry-after", "1"),
            )
            .mount(&mock_server)
            .await;

        let pool = Arc::new(MockSessionPort::healthy());
        let client = HttpClient::builder(HttpClientConfig {
            max_retries: 1,
            backoff_base_ms: 10,
            backoff_max_ms: 50,
            ..Default::default()
        })
        .session_pool(pool.clone())
        .build()
        .unwrap();

        let result = client.get(&mock_server.uri()).await;

        assert!(result.is_err());
        assert_eq!(pool.failure_count(), 1, "report_failure should be called once");
        assert_eq!(
            pool.last_failure_status(),
            Some(429),
            "Failure status should be 429 (HIGH penalty)"
        );
    }

    // ── Test: no session pool → no DomainBanned error ──

    #[tokio::test]
    async fn test_no_pool_no_domain_banned() {
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();

        // Without a session pool, even a valid URL should not return DomainBanned
        let result = client.get("not-a-valid-url").await;
        assert!(result.is_err());
        assert!(
            !matches!(result.unwrap_err(), HttpError::DomainBanned(_)),
            "Should not return DomainBanned when no pool is configured"
        );
    }

    // ── Test: 404 response → no report_failure (zero penalty) ──

    #[tokio::test]
    async fn test_404_no_penalty_no_failure_report() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let pool = Arc::new(MockSessionPort::healthy());
        let client = client_with_pool(pool.clone());

        let result = client.get(&format!("{}/missing", mock_server.uri())).await;

        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), HttpError::ClientError(404)),
            "Should return ClientError(404)"
        );
        assert_eq!(
            pool.failure_count(),
            0,
            "404 should NOT trigger report_failure (zero penalty)"
        );
    }
}
