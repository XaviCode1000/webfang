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

use anyhow::{Context, Result};
use tracing::debug;
use wreq::Client;
use wreq_util::Profile;

use crate::domain::http_config::HttpClientConfig;
use crate::domain::{CrawlError, CrawlerConfig};
use crate::infrastructure::http::create_http_client_with_config;

/// Create a rate-limited HTTP client
///
/// Delegates to the shared config-driven factory ([`create_http_client_with_config`],
/// the #299 source of truth) so the crawl client carries the same Chrome Client
/// Hints, pooled user-agent, pool tuning, compression, cookie store, and redirect
/// policy as the scrape client — with the TLS/H2 fingerprint resolved from config
/// instead of a hardcoded `Chrome145` (#312).
///
/// # Arguments
///
/// * `delay_ms` - Delay between requests in milliseconds
/// * `tls_emulation` - TLS/HTTP2 fingerprint preset applied to the client
///
/// # Returns
///
/// Configured wreq Client
///
/// # Errors
///
/// Returns an error if the underlying wreq client fails to build.
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::crawler::create_rate_limited_client;
/// use wreq_util::Profile;
///
/// let client = create_rate_limited_client(500, Profile::Chrome145).unwrap();
/// ```
pub fn create_rate_limited_client(delay_ms: u64, tls_emulation: Profile) -> Result<Client> {
    // The connect timeout replicates the historical 10s cap of this client; the
    // per-request timeout is applied by `fetch_url` from `CrawlerConfig`.
    let http_config = HttpClientConfig {
        tls_emulation,
        connect_timeout_secs: 10,
        ..Default::default()
    };
    let client = create_http_client_with_config(&http_config)
        .context("failed to build rate-limited HTTP client")?;

    debug!(
        "Created rate-limited HTTP client with delay_ms={} tls_emulation={:?}",
        delay_ms, tls_emulation
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
pub async fn fetch_url(url: &str, config: &CrawlerConfig) -> Result<String, CrawlError> {
    debug!("Fetching URL: {}", url);

    let client = create_rate_limited_client(config.delay_ms, config.tls_emulation)
        .map_err(|e| CrawlError::Internal(format!("Failed to create HTTP client: {e}")))?;

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

    let text = response.text().await.map_err(|e| CrawlError::Network {
        message: e.to_string(),
        status_code: None,
    })?;

    Ok(text)
}

#[cfg(test)]
#[cfg(not(miri))] // all tests create wreq::Client with boring-sys2 FFI (unsupported by Miri)
mod tests {
    use super::*;

    #[test]
    fn test_create_rate_limited_client() {
        let client = create_rate_limited_client(500, Profile::Chrome145);
        assert!(client.is_ok());
    }

    #[test]
    fn test_create_rate_limited_client_zero_delay() {
        let client = create_rate_limited_client(0, Profile::Chrome145);
        assert!(client.is_ok());
    }

    #[test]
    fn test_create_rate_limited_client_honors_profile_param() {
        // The profile parameter must be accepted and threaded into the client
        // builder for every preset, not just the Chrome145 default (#312).
        for profile in [Profile::Chrome145, Profile::Chrome131, Profile::Firefox135] {
            assert!(
                create_rate_limited_client(100, profile).is_ok(),
                "client build should succeed for profile {profile:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_fetch_url_with_custom_profile_succeeds() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>ok</html>"))
            .mount(&server)
            .await;

        let seed = url::Url::parse(&server.uri()).unwrap();
        let config = CrawlerConfig::builder(seed)
            .tls_emulation(Profile::Chrome131)
            .build();

        let html = fetch_url(&server.uri(), &config)
            .await
            .expect("fetch_url should succeed with a custom TLS profile");
        assert_eq!(html, "<html>ok</html>");
    }
}
