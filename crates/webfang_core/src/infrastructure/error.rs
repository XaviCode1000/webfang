//! Infrastructure layer errors
//!
//! Errors for HTTP, network, persistence, and other infrastructure concerns.

use thiserror::Error;

/// Infrastructure-level errors representing external system failures.
///
/// These errors represent failures in infrastructure components (HTTP,
/// persistence, ingestion) that are independent of business logic.
#[derive(Error, Debug)]
pub enum InfraError {
    /// HTTP request failed with a status code
    #[error("http error {status} al acceder a {url}")]
    Http {
        /// The HTTP status code
        status: u16,
        /// The URL that was being accessed
        url: String,
    },

    /// Network error (connection failed, timeout, etc.)
    #[error("error de red: {0}")]
    Network(String),

    /// Middleware error (from reqwest-middleware, e.g., retry failures)
    #[error("error de middleware: {0}")]
    Middleware(String),

    /// WAF/CAPTCHA challenge detected in HTTP 200 response
    #[error("WAF/CAPTCHA detectado en {url}: {provider}")]
    WafBlocked {
        /// URL that was blocked
        url: String,
        /// Detected WAF provider (e.g., "Cloudflare", "DataDome", "reCAPTCHA")
        provider: String,
    },

    /// Asset download failed
    #[error("Error de descarga: {0}")]
    Download(String),

    /// Global download timeout (30s)
    #[error("descarga superó tiempo global de 30 segundos")]
    GlobalTimeout,

    /// Slowloris attack detected (per-chunk timeout)
    #[error("descarga superó timeout de inactividad de 5 segundos por chunk")]
    SlowlorisTimeout,

    /// Payload exceeded 25MB limit
    #[error("recurso superó límite de 25 MB")]
    PayloadTooLarge,

    /// Semaphore exhausted (backpressure)
    #[error("semáforo agotado: no hay permisos disponibles")]
    SemaphoreInanition,

    /// Persistence error (SQLite storage layer)
    #[error("Error de persistencia: {0}")]
    Persistence(String),

    /// Elastic ingestion pipeline error
    #[error("Error de ingestión: {0}")]
    Ingestion(String),

    /// HTTP/2 configuration error (ALPN, settings, or handshake failure)
    #[error("Error de configuración HTTP/2: {0}")]
    H2Config(String),

    /// URL parse error
    #[error("Error de parseo de URL: {0}")]
    UrlParse(#[from] url::ParseError),

    /// I/O error (file system, etc.)
    #[error("Error de I/O: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infra_error_http() {
        let err = InfraError::Http {
            status: 404,
            url: "https://example.com".to_string(),
        };
        assert!(err.to_string().contains("404"));
        assert!(err.to_string().contains("example.com"));
    }

    #[test]
    fn test_infra_error_network() {
        let err = InfraError::Network("connection refused".to_string());
        assert!(err.to_string().contains("connection refused"));
        assert!(err.to_string().contains("red"));
    }

    #[test]
    fn test_infra_error_waf_blocked() {
        let err = InfraError::WafBlocked {
            url: "https://example.com".to_string(),
            provider: "Cloudflare".to_string(),
        };
        assert!(err.to_string().contains("example.com"));
        assert!(err.to_string().contains("Cloudflare"));
    }

    #[test]
    fn test_infra_error_global_timeout() {
        let err = InfraError::GlobalTimeout;
        assert!(err.to_string().contains("30 segundos"));
    }

    #[test]
    fn test_infra_error_is_std_error() {
        let err = InfraError::Network("test".to_string());
        let _: &dyn std::error::Error = &err;
    }
}
