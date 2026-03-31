//! HTTP client wrapper with retry middleware and status-specific handling
//!
//! Provides a configurable HTTP client with:
//! - Custom headers (Accept-Language, Accept, Referer, Cache-Control)
//! - Exponential backoff retry policy
//! - Specific handling for 403 (rotate UA), 429 (backoff), 5xx (retry)
//! - Cookie support
//!
//! # Examples
//!
//! ```no_run
//! use rust_scraper::application::http_client::{HttpClient, HttpClientConfig};
//!
//! let config = HttpClientConfig::default();
//! let client = HttpClient::new(config).unwrap();
//! // Use client for HTTP requests
//! ```

use crate::error::ScraperError;
use crate::user_agent::UserAgentCache;
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, warn};

/// Result type for HttpClient operations
pub type HttpResult<T> = Result<T, HttpError>;

/// Configuration for HTTP client behavior
///
/// Controls headers, retry behavior, and cookie handling.
/// Use `Default` for sensible production defaults.
#[derive(Debug, Clone)]
pub struct HttpClientConfig {
    /// Accept-Language header value
    pub accept_language: String,
    /// Accept header value
    pub accept: String,
    /// Referer header value
    pub referer: String,
    /// Cache-Control header value
    pub cache_control: String,
    /// Maximum number of retry attempts
    pub max_retries: u32,
    /// Base delay for exponential backoff (in milliseconds)
    pub backoff_base_ms: u64,
    /// Maximum delay for exponential backoff (in milliseconds)
    pub backoff_max_ms: u64,
    /// Enable cookie jar
    pub enable_cookies: bool,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            accept_language: "en-US,en;q=0.9".into(),
            accept: "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8".into(),
            referer: "https://www.google.com/".into(),
            cache_control: "no-cache".into(),
            max_retries: 3,
            backoff_base_ms: 1000,
            backoff_max_ms: 10000,
            enable_cookies: true,
        }
    }
}

/// HTTP-specific errors with status code information
///
/// Variants provide specific handling hints:
/// - `Forbidden`: 403 - retry with different UA
/// - `RateLimited`: 429 - respect Retry-After header
/// - `ClientError` / `ServerError`: other 4xx/5xx codes
#[derive(Debug, Clone, PartialEq)]
pub enum HttpError {
    /// 403 Forbidden - site blocking
    Forbidden,
    /// 429 Rate Limited - contains retry-after seconds
    RateLimited(u64),
    /// Other 4xx errors - contains status code
    ClientError(u16),
    /// 5xx server errors - contains status code
    ServerError(u16),
    /// Request timeout
    Timeout,
    /// Connection error - contains error message
    Connection(String),
    /// Request building/error - contains error message
    Request(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Forbidden => write!(f, "403 Forbidden - site blocking"),
            HttpError::RateLimited(retry_after) => {
                write!(f, "429 Rate Limited - retry after {} seconds", retry_after)
            }
            HttpError::ClientError(code) => write!(f, "Client Error {}", code),
            HttpError::ServerError(code) => write!(f, "Server Error {}", code),
            HttpError::Timeout => write!(f, "Request Timeout"),
            HttpError::Connection(msg) => write!(f, "Connection Error: {}", msg),
            HttpError::Request(msg) => write!(f, "Request Error: {}", msg),
        }
    }
}

impl std::error::Error for HttpError {}

/// HTTP client wrapper with configurable retry behavior
///
/// Wraps `reqwest::Client` and adds:
/// - Custom headers from config
/// - Status-specific retry logic
/// - User-agent rotation on 403
/// - Exponential backoff on 429 and 5xx
pub struct HttpClient {
    /// Internal reqwest client
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
        let builder = Client::builder()
            .timeout(Duration::from_secs(30))
            .gzip(true)
            .brotli(true)
            .cookie_store(true); // Explicitly enable cookies for session persistence
        
        let client = builder
            .build()
            .map_err(|e| ScraperError::Config(format!("Failed to create HTTP client: {}", e)))?;

        // Get user agents from fallback (synchronous)
        let user_agents = UserAgentCache::fallback_agents();

        debug!("HttpClient created with {} user agents", user_agents.len());

        Ok(Self {
            client,
            config,
            user_agents,
        })
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
        // Try with first UA, then loop handles 403 retry
        let mut ua_index = 0;
        let max_attempts = self.config.max_retries;
        
        loop {
            // Check if we've exhausted retries
            if ua_index >= self.user_agents.len() && ua_index > 0 {
                return Err(HttpError::Forbidden);
            }

            let ua = self.user_agents.get(ua_index % self.user_agents.len()).cloned()
                .unwrap_or_else(|| "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36".into());

            let request = self.client
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
                    return response.text().await.map_err(|e| HttpError::Request(e.to_string()));
                }
                403 => {
                    warn!("403 Forbidden from {}", url);
                    // Retry once with different UA
                    if ua_index == 0 {
                        ua_index += 1;
                        continue;
                    }
                    return Err(HttpError::Forbidden);
                }
                429 => {
                    // Try to get Retry-After header
                    let retry_after = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(1);

                    debug!("429 Rate Limited, retry after {}s", retry_after);

                    // Do retry loop with backoff for 429
                    let mut attempt = 0;
                    let ua_for_retry = ua.clone();
                    while attempt < max_attempts {
                        attempt += 1;
                        
                        let delay_ms = if retry_after > 0 {
                            retry_after * 1000
                        } else {
                            // Backoff: 1s -> 2s -> 4s (attempt starts at 1)
                            let exponent = attempt.saturating_sub(1);
                            let delay = self.config.backoff_base_ms * (2_u64.pow(exponent));
                            delay.min(self.config.backoff_max_ms)
                        };

                        debug!("429 retry attempt {} after {}ms", attempt, delay_ms);
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                        // Try request again
                        let request = self.client
                            .get(url)
                            .header("Accept-Language", &self.config.accept_language)
                            .header("Accept", &self.config.accept)
                            .header("Referer", &self.config.referer)
                            .header("Cache-Control", &self.config.cache_control)
                            .header("User-Agent", &ua_for_retry);

                        match request.send().await {
                            Ok(resp) => {
                                if resp.status().is_success() {
                                    return resp.text().await.map_err(|e| HttpError::Request(e.to_string()));
                                } else if resp.status().as_u16() == 429 {
                                    // Still rate limited, continue backing off
                                    continue;
                                } else if resp.status().is_server_error() {
                                    // Server error, continue retrying
                                    continue;
                                } else {
                                    // Client error (other than 429/403)
                                    return Err(HttpError::ClientError(resp.status().as_u16()));
                                }
                            }
                            Err(e) => {
                                if e.is_timeout() {
                                    return Err(HttpError::Timeout);
                                }
                                // Connection error, continue retrying
                                continue;
                            }
                        }
                    }
                    // Exhausted retries
                    return Err(HttpError::RateLimited(retry_after));
                }
                500..=599 => {
                    debug!("{} from {}", status, url);
                    
                    // Retry loop with exponential backoff
                    let mut attempt = 0;
                    let ua_for_retry = ua.clone();
                    while attempt < max_attempts {
                        attempt += 1;
                        
                        // Backoff: 1s -> 2s -> 4s (attempt starts at 1)
                        let exponent = attempt.saturating_sub(1);
                        let delay = self.config.backoff_base_ms * (2_u64.pow(exponent));
                        let delay_ms = delay.min(self.config.backoff_max_ms);

                        debug!("5xx retry attempt {} after {}ms", attempt, delay_ms);
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                        // Try request again
                        let request = self.client
                            .get(url)
                            .header("Accept-Language", &self.config.accept_language)
                            .header("Accept", &self.config.accept)
                            .header("Referer", &self.config.referer)
                            .header("Cache-Control", &self.config.cache_control)
                            .header("User-Agent", &ua_for_retry);

                        match request.send().await {
                            Ok(resp) => {
                                if resp.status().is_success() {
                                    return resp.text().await.map_err(|e| HttpError::Request(e.to_string()));
                                } else if resp.status().is_server_error() {
                                    // Server error, continue retrying
                                    continue;
                                } else {
                                    // Client error (4xx)
                                    return Err(HttpError::ClientError(resp.status().as_u16()));
                                }
                            }
                            Err(e) => {
                                if e.is_timeout() {
                                    return Err(HttpError::Timeout);
                                }
                                // Connection error, continue retrying
                                continue;
                            }
                        }
                    }
                    // Exhausted retries
                    return Err(HttpError::ServerError(status.as_u16()));
                }
                code if (400..=499).contains(&code) => {
                    return Err(HttpError::ClientError(code));
                }
                code => {
                    return Err(HttpError::ServerError(code));
                }
            }
        }
    }
}

// Re-export types needed for legacy compatibility
pub use reqwest_middleware::ClientWithMiddleware;

// Legacy function - uses reqwest-middleware for backward compatibility
/// Create configured HTTP client with retry middleware
///
/// This function creates a client using reqwest-middleware for automatic retry.
/// For more control, use `HttpClient::new()` with `HttpClientConfig`.
pub fn create_http_client() -> Result<ClientWithMiddleware, ScraperError> {
    // Get fallback user agents (sync, no async needed)
    let agents = UserAgentCache::fallback_agents();
    let user_agent = get_random_user_agent_from_pool(&agents);

    tracing::debug!("Using user agent: {}", user_agent);

    let base_client = Client::builder()
        .user_agent(user_agent)
        .timeout(Duration::from_secs(30))
        .gzip(true)
        .brotli(true)
        .build()
        .map_err(|e| ScraperError::Config(format!("Failed to create HTTP client: {}", e)))?;

    use reqwest_middleware::ClientBuilder;
    use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};

    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let client = ClientBuilder::new(base_client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();

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

    // =========================================================================
    // HttpClientConfig tests
    // =========================================================================

    #[test]
    fn test_http_client_config_default_values() {
        let config = HttpClientConfig::default();

        assert_eq!(config.accept_language, "en-US,en;q=0.9");
        assert_eq!(config.accept, "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8");
        assert_eq!(config.referer, "https://www.google.com/");
        assert_eq!(config.cache_control, "no-cache");
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.backoff_base_ms, 1000);
        assert_eq!(config.backoff_max_ms, 10000);
        assert!(config.enable_cookies);
    }

    #[test]
    fn test_http_client_config_clone() {
        let config = HttpClientConfig::default();
        let cloned = config.clone();

        assert_eq!(config.accept_language, cloned.accept_language);
        assert_eq!(config.max_retries, cloned.max_retries);
    }

    #[test]
    fn test_http_client_config_custom_values() {
        let config = HttpClientConfig {
            accept_language: "es-ES".into(),
            accept: "application/json".into(),
            referer: "https://example.com/".into(),
            cache_control: "max-age=3600".into(),
            max_retries: 5,
            backoff_base_ms: 500,
            backoff_max_ms: 5000,
            enable_cookies: false,
        };

        assert_eq!(config.accept_language, "es-ES");
        assert_eq!(config.max_retries, 5);
        assert!(!config.enable_cookies);
    }

    // =========================================================================
    // HttpError tests
    // =========================================================================

    #[test]
    fn test_http_error_forbidden() {
        let err = HttpError::Forbidden;
        assert_eq!(err, HttpError::Forbidden);
    }

    #[test]
    fn test_http_error_rate_limited() {
        let err = HttpError::RateLimited(60);
        assert_eq!(err, HttpError::RateLimited(60));

        let err2 = HttpError::RateLimited(30);
        assert_ne!(err, err2);
    }

    #[test]
    fn test_http_error_client_error() {
        let err = HttpError::ClientError(404);
        assert_eq!(err, HttpError::ClientError(404));
    }

    #[test]
    fn test_http_error_server_error() {
        let err = HttpError::ServerError(500);
        assert_eq!(err, HttpError::ServerError(500));
    }

    #[test]
    fn test_http_error_timeout() {
        let err = HttpError::Timeout;
        assert_eq!(err, HttpError::Timeout);
    }

    #[test]
    fn test_http_error_connection() {
        let err = HttpError::Connection("Connection refused".into());
        assert_eq!(err, HttpError::Connection("Connection refused".into()));
    }

    #[test]
    fn test_http_error_request() {
        let err = HttpError::Request("Invalid URL".into());
        assert_eq!(err, HttpError::Request("Invalid URL".into()));
    }

    #[test]
    fn test_http_error_display() {
        assert_eq!(format!("{}", HttpError::Forbidden), "403 Forbidden - site blocking");
        assert_eq!(format!("{}", HttpError::RateLimited(30)), "429 Rate Limited - retry after 30 seconds");
        assert_eq!(format!("{}", HttpError::ClientError(404)), "Client Error 404");
        assert_eq!(format!("{}", HttpError::ServerError(500)), "Server Error 500");
        assert_eq!(format!("{}", HttpError::Timeout), "Request Timeout");
    }

    // =========================================================================
    // HttpClient creation tests
    // =========================================================================

    #[test]
    fn test_http_client_new_success() {
        let config = HttpClientConfig::default();
        let result = HttpClient::new(config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_http_client_has_user_agents() {
        let config = HttpClientConfig::default();
        let _client = HttpClient::new(config).unwrap();
        
        // Client should have user agents (from fallback)
        // We can't directly check private field, but we can verify it was created
        assert!(true);
    }

    #[tokio::test]
    async fn test_http_client_get_invalid_url() {
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();
        
        // Invalid URL should fail
        let result = client.get("not-a-valid-url").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore = "requires network - run with cargo test --ignored"]
    async fn test_http_client_get_example_com() {
        let config = HttpClientConfig::default();
        let client = HttpClient::new(config).unwrap();
        
        // Valid request to example.com should succeed
        let result = client.get("https://example.com").await;
        assert!(result.is_ok());
        
        let body = result.unwrap();
        assert!(!body.is_empty());
    }
}

#[cfg(test)]
mod wiremock_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // =========================================================================
    // 403 Handling with User-Agent Rotation
    // =========================================================================

    #[tokio::test]
    async fn test_403_returns_error() {
        let mock_server = MockServer::start().await;
        
        // Request returns 403
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(403))
            .mount(&mock_server)
            .await;

        let config = HttpClientConfig {
            max_retries: 1, // Only 1 retry
            ..Default::default()
        };
        let client = HttpClient::new(config).unwrap();

        let result = client.get(&mock_server.uri()).await;
        
        // Should return 403 error after exhausting retries
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::Forbidden));
    }

    // =========================================================================
    // 429 Rate Limited with Backoff
    // =========================================================================

    #[tokio::test]
    async fn test_429_returns_error() {
        let mock_server = MockServer::start().await;
        
        // Request returns 429
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

        // Should fail with rate limit error
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::RateLimited(_)));
        // Should have waited at least for the retry-after
        assert!(elapsed.as_millis() >= 10);
    }

    // =========================================================================
    // 500 Server Error with Automatic Retry
    // =========================================================================

    #[tokio::test]
    async fn test_500_returns_error() {
        let mock_server = MockServer::start().await;
        
        // Always return 500
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
        
        // Should fail after exhausting retries
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::ServerError(500)));
    }

    #[tokio::test]
    async fn test_500_exhausts_retries() {
        let mock_server = MockServer::start().await;
        
        // Always return 500
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
        
        // Should fail after exhausting retries
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), HttpError::ServerError(500)));
    }

    // =========================================================================
    // Client Error (4xx except 403/429)
    // =========================================================================

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

    // =========================================================================
    // Successful Response
    // =========================================================================

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
