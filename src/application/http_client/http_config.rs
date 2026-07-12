//! Configuration for HTTP client behavior
//!
//! Controls headers, retry behavior, and cookie handling.
//! Use `Default` for sensible production defaults.

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
    /// Request timeout in seconds
    pub timeout_secs: u64,
    /// Connection timeout in seconds
    pub connect_timeout_secs: u64,
    /// Rate limit: requests per minute (None for no limit)
    pub rate_limit_rpm: Option<u32>,
    /// TLS fingerprint emulation preset
    pub tls_emulation: wreq_util::Profile,
    /// Custom User-Agent override
    pub user_agent: Option<String>,
    /// H2/TLS profile name (e.g. "Chrome145", "Chrome131").
    /// Mapped to `tls_emulation` on construction.
    pub h2_profile: String,
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
            timeout_secs: 30,
            connect_timeout_secs: 10,
            rate_limit_rpm: None,
            tls_emulation: wreq_util::Profile::Chrome145,
            user_agent: None,
            h2_profile: "Chrome145".to_owned(),
        }
    }
}

impl HttpClientConfig {
    /// Resolve an H2 profile name to a `wreq_util::Profile`.
    ///
    /// Falls back to Chrome145 for unknown names.
    #[must_use]
    pub fn resolve_profile(name: &str) -> wreq_util::Profile {
        match name {
            "Chrome131" => wreq_util::Profile::Chrome131,
            "Chrome145" => wreq_util::Profile::Chrome145,
            _ => {
                tracing::warn!("Unknown H2 profile '{name}', falling back to Chrome145");
                wreq_util::Profile::Chrome145
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_client_config_default_values() {
        let config = HttpClientConfig::default();

        assert_eq!(config.accept_language, "en-US,en;q=0.9");
        assert_eq!(
            config.accept,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
        );
        assert_eq!(config.referer, "https://www.google.com/");
        assert_eq!(config.cache_control, "no-cache");
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.backoff_base_ms, 1000);
        assert_eq!(config.backoff_max_ms, 10000);
        assert!(config.enable_cookies);
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.connect_timeout_secs, 10);
        assert_eq!(config.rate_limit_rpm, None);
        assert_eq!(config.tls_emulation, wreq_util::Profile::Chrome145);
        assert_eq!(config.user_agent, None);
        assert_eq!(config.h2_profile, "Chrome145");
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
            timeout_secs: 60,
            connect_timeout_secs: 20,
            rate_limit_rpm: Some(30),
            tls_emulation: wreq_util::Profile::Chrome131,
            user_agent: Some("custom".into()),
            h2_profile: "Chrome131".to_owned(),
        };

        assert_eq!(config.accept_language, "es-ES");
        assert_eq!(config.max_retries, 5);
        assert!(!config.enable_cookies);
        assert_eq!(config.timeout_secs, 60);
        assert_eq!(config.connect_timeout_secs, 20);
        assert_eq!(config.rate_limit_rpm, Some(30));
        assert_eq!(config.tls_emulation, wreq_util::Profile::Chrome131);
        assert_eq!(config.h2_profile, "Chrome131");
    }

    #[test]
    fn test_resolve_profile_known_names() {
        assert_eq!(
            HttpClientConfig::resolve_profile("Chrome131"),
            wreq_util::Profile::Chrome131
        );
        assert_eq!(
            HttpClientConfig::resolve_profile("Chrome145"),
            wreq_util::Profile::Chrome145
        );
    }

    #[test]
    fn test_resolve_profile_unknown_falls_back() {
        assert_eq!(
            HttpClientConfig::resolve_profile("UnknownProfile"),
            wreq_util::Profile::Chrome145
        );
    }
}
