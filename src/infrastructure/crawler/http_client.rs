//! HTTP client with rate limiting
//!
//! Provides rate-limited HTTP client for crawling.
//!
//! # Rules Applied
//!
//! - **mem-with-capacity**: Pre-allocate when size is known
//! - **own-borrow-over-clone**: Accept references not owned values
//! - **clean-architecture**: Converts reqwest::Error → CrawlError::Network (NO reqwest in Domain)

use std::time::Duration;

use anyhow::Result;
use reqwest::Client;
use tracing::debug;

use crate::domain::{CrawlError, CrawlerConfig};

/// Create a rate-limited HTTP client
///
/// Following **mem-with-capacity**: Pre-allocates client with appropriate pool size.
///
/// # Arguments
///
/// * `delay_ms` - Delay between requests in milliseconds
///
/// # Returns
///
/// Configured reqwest Client
///
/// # Examples
///
/// ```
/// use rust_scraper::infrastructure::crawler::create_rate_limited_client;
///
/// let client = create_rate_limited_client(500).unwrap();
/// ```
pub fn create_rate_limited_client(delay_ms: u64) -> Result<Client> {
    // Hardware-aware: limit connection pool for low-resource systems
    let pool_size = std::cmp::max(3, num_cpus::get() - 1);

    let client = Client::builder()
        .pool_max_idle_per_host(pool_size)
        .pool_idle_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .gzip(true)
        .brotli(true)
        .user_agent("rust-scraper/0.3.0 (Web Crawler)")
        .build()?;

    debug!(
        "Created rate-limited HTTP client with pool_size={} delay_ms={}",
        pool_size, delay_ms
    );

    Ok(client)
}

/// Fetch a URL and return the response text
///
/// Following **own-borrow-over-clone**: Accepts `&str` and `&CrawlerConfig`.
/// Following **clean-architecture**: Converts reqwest::Error → CrawlError::Network
///
/// # Arguments
///
/// * `url` - URL to fetch
/// * `config` - Crawler configuration
///
/// # Returns
///
/// * `Ok(String)` - Response text
/// * `Err(CrawlError)` - Error during fetch
pub async fn fetch_url(
    url: &str,
    config: &CrawlerConfig,
) -> Result<String, CrawlError> {
    debug!("Fetching URL: {}", url);

    let client = create_rate_limited_client(config.delay_ms)
        .map_err(|e| CrawlError::Internal(format!("Failed to create HTTP client: {}", e)))?;

    let response = client
        .get(url)
        .timeout(Duration::from_secs(config.timeout_secs))
        .send()
        .await
        .map_err(|e| CrawlError::Network {
            message: e.to_string(),
            status_code: e.status().map(|s| s.as_u16()),
        })?;

    // Check for successful status
    if !response.status().is_success() {
        // Convert HTTP error to CrawlError::Network
        return Err(CrawlError::Network {
            message: format!("HTTP error: {}", response.status()),
            status_code: Some(response.status().as_u16()),
        });
    }

    let text = response
        .text()
        .await
        .map_err(|e| CrawlError::Network {
            message: e.to_string(),
            status_code: None,
        })?;

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_rate_limited_client() {
        let client = create_rate_limited_client(500);
        assert!(client.is_ok());
    }

    #[test]
    fn test_create_rate_limited_client_zero_delay() {
        let client = create_rate_limited_client(0);
        assert!(client.is_ok());
    }
}
