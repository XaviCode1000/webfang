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
}
