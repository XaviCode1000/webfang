//! HTTP client creation with retry middleware
//!
//! Creates a configured reqwest client with:
//! - User-Agent rotation (anti-bot evasion)
//! - Exponential backoff retry policy
//! - Gzip/brotli compression support
//! - Explicit timeout

use crate::error::{Result, ScraperError};
use crate::user_agent;
use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use std::time::Duration;

/// Timeout for HTTP requests in seconds
const TIMEOUT_SECS: u64 = 30;

/// Create configured HTTP client with retry middleware and user-agent rotation
///
/// Uses exponential backoff for transient failures:
/// - 3 retries by default
/// - Exponential backoff: 100ms → 200ms → 400ms
/// - Retries on: 5xx errors, timeouts, connection errors
///
/// # Examples
///
/// ```no_run
/// use rust_scraper::application::create_http_client;
///
/// let client = create_http_client().unwrap();
/// // Use client for HTTP requests
/// ```
pub fn create_http_client() -> Result<ClientWithMiddleware> {
    let base_client = Client::builder()
        .user_agent(user_agent::random_user_agent())
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .gzip(true)
        .brotli(true)
        .build()
        .map_err(|e| ScraperError::Config(format!("Failed to create HTTP client: {}", e)))?;

    let retry_policy = ExponentialBackoff::builder().build_with_max_retries(3);
    let client = ClientBuilder::new(base_client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build();

    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_http_client_success() {
        let result = create_http_client();
        assert!(result.is_ok());
    }
}
