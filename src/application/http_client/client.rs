//! HTTP client implementation
//!
//! Wraps `wreq::Client` with retry logic, UA rotation, and WAF detection.

use super::config::HttpClientConfig;
use super::error::{HttpError, HttpResult};
use super::waf::detect_waf_challenge;
use crate::error::ScraperError;
use crate::user_agent::UserAgentCache;
use std::time::Duration;
use tracing::{debug, warn};
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
pub struct HttpClient {
    /// Internal wreq client
    client: Client,
    /// Configuration for headers and retry
    config: HttpClientConfig,
    /// Pool of user agents for rotation
    user_agents: Vec<String>,
}

impl HttpClient {
    /// Create a new HTTP client with the given configuration
    ///
    /// # Errors
    ///
    /// Returns `ScraperError::Config` if client creation fails
    pub fn new(config: HttpClientConfig) -> Result<Self, ScraperError> {
        let pool_size = std::cmp::max(3, num_cpus::get() - 1);

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

        let builder = Client::builder()
            .emulation(Emulation::Chrome145)
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(pool_size)
            .pool_idle_timeout(Duration::from_secs(60))
            .gzip(true)
            .brotli(true)
            .cookie_store(true)
            .redirect(wreq::redirect::Policy::limited(10));

        let client = builder
            .build()
            .map_err(|e| ScraperError::Config(format!("failed to create http client: {}", e)))?;

        let user_agents = UserAgentCache::fallback_agents();

        debug!("HttpClient created with {} user agents", user_agents.len());

        Ok(Self {
            client,
            config,
            user_agents,
        })
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
    /// Returns `HttpError` for failed requests
    pub async fn get(&self, url: &str) -> HttpResult<String> {
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

            let request = self
                .client
                .get(url)
                .header("Accept-Language", &self.config.accept_language)
                .header("Accept", &self.config.accept)
                .header("Referer", &self.config.referer)
                .header("Cache-Control", &self.config.cache_control)
                .header("User-Agent", ua.clone());

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
                    let body = response
                        .text()
                        .await
                        .map_err(|e| HttpError::Request(e.to_string()))?;

                    if let Some(provider) = detect_waf_challenge(&body) {
                        warn!(
                            "WAF challenge detected from {} ({}), rotating UA",
                            url, provider
                        );
                        if ua_index == 0 {
                            ua_index += 1;
                            continue;
                        }
                        return Err(HttpError::WafChallenge(provider.to_string()));
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
                                } else if resp.status().as_u16() == 429 {
                                    continue;
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
                    return Err(HttpError::RateLimited(retry_after));
                },
                500..=599 => {
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
                    return Err(HttpError::ClientError(code));
                },
                code => {
                    return Err(HttpError::ServerError(code));
                },
            }
        }
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
        .map_err(|e| ScraperError::Config(format!("failed to create http client: {}", e)))?;

    Ok(client)
}

/// Get random user agent from pool (legacy function)
pub fn get_random_user_agent_from_pool(pool: &[String]) -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let index = rng.gen_range(0..pool.len());
    pool[index].clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::http_client::config::HttpClientConfig;

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
mod wiremock_tests {
    use super::*;
    use crate::application::http_client::config::HttpClientConfig;
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
mod waf_detection_tests {
    use super::*;
    use crate::application::http_client::config::HttpClientConfig;
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
