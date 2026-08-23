//! Bare `wreq::Client` factories shared across the crate.
//!
//! These factories build the raw inner [`Client`] — Chrome Client Hints +
//! Sec-Fetch default headers, the resolved H2/TLS profile, request/connect
//! timeouts, connection-pool tuning, compression, cookie storage and a bounded
//! redirect policy — **without** the retry / UA-rotation / WAF middleware that
//! [`HttpClient`](crate::application::http_client::HttpClient) layers on top.
//! Callers that manage requests themselves (the crawler, sitemap discovery,
//! URL validation, preflight) use these; callers that want the hardened,
//! retrying behavior use [`HttpClient::new`](crate::application::http_client::HttpClient::new).
//!
//! [`build_wreq_client`] is the single construction point shared by
//! [`HttpClient::new`](crate::application::http_client::HttpClient::new) and
//! the bare-client factories here, so both paths build an identical stack.

use crate::domain::http_config::HttpClientConfig;
use crate::error::ScraperError;
use crate::infrastructure::user_agent::UserAgentCache;
use std::time::Duration;
use wreq::header::{HeaderMap, HeaderName, HeaderValue};
use wreq::Client;

/// Client Hints headers for Chrome 145 (2026 Standard)
/// These headers must match the TLS fingerprint to avoid "Headless Spoofing" detection
const CLIENT_HINTS_SEC_CH_UA: &str =
    "\"Google Chrome\";v=\"145\", \"Chromium\";v=\"145\", \"Not=A?Brand\";v=\"99\"";
const CLIENT_HINTS_SEC_CH_UA_MOBILE: &str = "?0";
const CLIENT_HINTS_SEC_CH_UA_PLATFORM: &str = "\"Linux\"";

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
pub(super) fn build_wreq_client(
    config: &HttpClientConfig,
    user_agent: Option<String>,
) -> Result<Client, ScraperError> {
    // Canonical detector seam (Q2): same "auto" as every other subsystem.
    let pool_size =
        std::cmp::max(6, crate::domain::budget::detector::system_parallelism().get() - 1);

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
        // SSRF guard (#703): default 10-hop limit + stops redirects that
        // target a literal forbidden IP. Hostname targets are validated at
        // entry by the async SSRF guard.
        .redirect(crate::infrastructure::ssrf::redirect_policy());

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
