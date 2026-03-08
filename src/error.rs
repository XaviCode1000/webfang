//! Error handling module for rust_scraper.
//!
//! Uses thiserror for library error types (err-thiserror-lib).
//! This provides type-safe, structured error handling instead of anyhow.

use thiserror::Error;

/// Main error type for the scraper library.
///
/// Each variant represents a specific failure mode, making it easy to:
/// - Handle specific errors in calling code
/// - Convert to/from other error types
/// - Provide meaningful error messages to users
#[derive(Error, Debug)]
pub enum ScraperError {
    /// URL is invalid or cannot be parsed
    #[error("URL inválida: {0}")]
    InvalidUrl(String),

    /// HTTP request failed with a status code
    #[error("HTTP error {status} al acceder a {url}")]
    Http {
        /// The HTTP status code
        status: reqwest::StatusCode,
        /// The URL that was being accessed
        url: String,
    },

    /// Content extraction failed (Readability algorithm)
    #[error("Error de legibilidad: {0}")]
    Readability(String),

    /// I/O error (file system, etc.)
    #[error("Error de I/O: {0}")]
    Io(#[from] std::io::Error),

    /// Network error (connection failed, timeout, etc.)
    #[error("Error de red: {0}")]
    Network(#[from] reqwest::Error),

    /// Serialization/Deserialization error (JSON, YAML, etc.)
    #[error("Error de serialización: {0}")]
    Serialization(#[from] serde_json::Error),

    /// YAML serialization error
    #[error("Error de YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// URL parse error
    #[error("Error de parseo de URL: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Asset extraction failed
    #[error("Error de extracción: {0}")]
    Extraction(String),

    /// Asset download failed
    #[error("Error de descarga: {0}")]
    Download(String),

    /// Configuration error
    #[error("Error de configuración: {0}")]
    Config(String),

    /// URL validation failed
    #[error("Validación de URL falló: {0}")]
    Validation(String),

    /// Conversion error (HTML to Markdown, YAML, etc.)
    #[error("Error de conversión: {0}")]
    Conversion(String),
}

/// Result type alias using ScraperError as the error type.
pub type Result<T> = std::result::Result<T, ScraperError>;

impl ScraperError {
    /// Create an InvalidUrl error
    #[must_use]
    pub fn invalid_url(msg: impl Into<String>) -> Self {
        Self::InvalidUrl(msg.into())
    }

    /// Create an Http error
    #[must_use]
    pub fn http(status: reqwest::StatusCode, url: &str) -> Self {
        Self::Http {
            status,
            url: url.to_string(),
        }
    }

    /// Create a Readability error
    #[must_use]
    pub fn readability(msg: impl Into<String>) -> Self {
        Self::Readability(msg.into())
    }

    /// Create an Extraction error
    #[must_use]
    pub fn extraction(msg: impl Into<String>) -> Self {
        Self::Extraction(msg.into())
    }

    /// Create a Download error
    #[must_use]
    pub fn download(msg: impl Into<String>) -> Self {
        Self::Download(msg.into())
    }

    /// Create a Conversion error
    #[must_use]
    pub fn conversion(msg: impl Into<String>) -> Self {
        Self::Conversion(msg.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_url_error() {
        let err = ScraperError::invalid_url("URL vacía");
        assert_eq!(err.to_string(), "URL inválida: URL vacía");
    }

    #[test]
    fn test_http_error() {
        let status = reqwest::StatusCode::from_u16(404).unwrap();
        let err = ScraperError::http(status, "https://example.com");
        assert!(err.to_string().contains("404"));
        assert!(err.to_string().contains("example.com"));
    }

    #[test]
    fn test_readability_error() {
        let err = ScraperError::readability("Failed to parse HTML");
        assert_eq!(
            err.to_string(),
            "Error de legibilidad: Failed to parse HTML"
        );
    }

    #[test]
    fn test_io_error_from_std() {
        let std_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err: ScraperError = std_err.into();
        assert!(err.to_string().contains("file not found"));
    }
}
