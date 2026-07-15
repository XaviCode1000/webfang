//! Wreq-based downloader implementation.
//!
//! Wraps a shared `wreq::Client` behind `Arc` for connection pooling.
//! Extracts cookies from responses and returns [`FetchedPage`] with HTML + cookies.
//!
//! Following **own-arc-shared**: Uses `Arc<Client>` for thread-safe shared ownership
//! of the connection pool. The client is created once and shared across all requests.

use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, instrument};
use url::Url;
use wreq::Client;
use wreq_util::Emulation;

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::DOWNLOAD_WREQ_LATENCY;
#[cfg(feature = "otel-metrics")]
use std::time::Instant;

use super::{Cookie, DownloadError, Downloader, FetchedPage};

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
/// ```no_run
/// use webfang::infrastructure::downloader::{WreqDownloader, Downloader};
///
/// # tokio_test::block_on(async {
/// let downloader = WreqDownloader::new(30, 10);
/// let page = downloader.fetch(&"https://example.com".parse().unwrap()).await.unwrap();
/// assert_eq!(page.status, 200);
/// # });
/// ```
pub struct WreqDownloader {
    client: Arc<Client>,
    timeout_secs: u64,
}

impl WreqDownloader {
    /// Create a new WreqDownloader with default Chrome 145 emulation.
    ///
    /// The client is built once and shared via `Arc` for connection pooling.
    ///
    /// # Arguments
    ///
    /// * `timeout_secs` - Request timeout in seconds
    /// * `connect_timeout_secs` - Connection timeout in seconds
    ///
    /// # Panics
    ///
    /// Panics if the wreq client cannot be built (should not happen with valid params).
    pub fn new(timeout_secs: u64, connect_timeout_secs: u64) -> Self {
        let pool_size = std::cmp::max(6, num_cpus::get() - 1);

        let client = Client::builder()
            .emulation(Emulation::Chrome145)
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(connect_timeout_secs))
            .pool_max_idle_per_host(pool_size)
            .pool_idle_timeout(Duration::from_secs(60))
            .gzip(true)
            .brotli(true)
            .cookie_store(true)
            .redirect(wreq::redirect::Policy::limited(10))
            .build()
            .expect("failed to build wreq client — this should not happen");

        debug!(
            "WreqDownloader created: pool_size={}, timeout={}s, connect_timeout={}s",
            pool_size, timeout_secs, connect_timeout_secs
        );

        Self {
            client: Arc::new(client),
            timeout_secs,
        }
    }

    /// Create a WreqDownloader from an existing `wreq::Client`.
    ///
    /// Useful when you need custom client configuration beyond the defaults.
    pub fn from_client(client: Client, timeout_secs: u64, _connect_timeout_secs: u64) -> Self {
        Self {
            client: Arc::new(client),
            timeout_secs,
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

impl Downloader for WreqDownloader {
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
    async fn fetch(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        debug!("Fetching URL: {}", url);

        #[cfg(feature = "otel-metrics")]
        let start = Instant::now();

        let response = self.client.get(url.as_str()).send().await.map_err(|e| {
            if e.is_timeout() {
                DownloadError::Timeout(self.timeout_secs)
            } else {
                DownloadError::Network(Box::new(e))
            }
        })?;

        let status = response.status().as_u16();

        // Check for non-2xx responses
        if !response.status().is_success() {
            let message = format!("HTTP {status}");
            return Err(DownloadError::Http { status, message });
        }

        // Extract cookies before consuming the response body
        let cookies = Self::extract_cookies(url, &response);

        // Extract the final URL after redirects
        let final_url = Url::parse(&response.uri().to_string())
            .map_err(|e| DownloadError::InvalidUrl(e.to_string()))?;

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

        let result = Ok(FetchedPage {
            url: final_url,
            html,
            status,
            cookies,
        });

        #[cfg(feature = "otel-metrics")]
        DOWNLOAD_WREQ_LATENCY.record(start.elapsed().as_secs_f64(), &[]);

        result
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        WREQ_MEMORY_COST
    }
}

#[cfg(test)]
#[cfg(not(miri))] // wreq uses boring-sys2 FFI (unsupported by Miri)
mod tests {
    use super::*;

    #[test]
    fn test_wreq_downloader_creation() {
        let downloader = WreqDownloader::new(30, 10);
        assert!(!downloader.supports_interactions());
        assert_eq!(downloader.memory_cost(), WREQ_MEMORY_COST);
    }

    #[test]
    fn test_wreq_downloader_from_client() {
        let client = Client::builder()
            .emulation(Emulation::Chrome145)
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
    async fn test_fetch_example_com() {
        let downloader = WreqDownloader::new(10, 5);
        let url: Url = "https://example.com".parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_ok());

        let page = result.unwrap();
        assert_eq!(page.status, 200);
        assert!(!page.html.is_empty());
        assert!(page.html.contains("Example Domain"));
    }
}

#[cfg(test)]
#[cfg(not(miri))]
mod wiremock_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_fetch_200_returns_body() {
        let mock_server = MockServer::start().await;
        let expected_body = "<html><body>Hello World</body></html>";

        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(expected_body))
            .mount(&mock_server)
            .await;

        let downloader = WreqDownloader::new(10, 5);
        let url: Url = mock_server.uri().parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_ok());

        let page = result.unwrap();
        assert_eq!(page.status, 200);
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

        let downloader = WreqDownloader::new(10, 5);
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

        let downloader = WreqDownloader::new(10, 5);
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

        let downloader = WreqDownloader::new(10, 5);
        let url: Url = format!("{}/redirect", mock_server.uri()).parse().unwrap();

        let result = downloader.fetch(&url).await;
        assert!(result.is_ok());

        let page = result.unwrap();
        assert!(page.url.as_str().contains("/target"));
    }
}

#[cfg(test)]
#[cfg(feature = "otel-metrics")]
mod metrics_tests {
    #[test]
    fn test_download_wreq_latency_instrument_init() {
        let _ = &*super::DOWNLOAD_WREQ_LATENCY;
    }
}
