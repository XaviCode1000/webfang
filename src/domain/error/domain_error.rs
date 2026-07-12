//! Domain-level errors representing business rule violations.
//!
//! Each variant carries English identifiers for programmatic matching
//! and Spanish `Display` messages for user-facing output.

use thiserror::Error;

/// Domain-level errors representing business rule violations.
///
/// These errors represent domain-level failures that are independent of
/// infrastructure or application concerns. Each variant maps to a
/// `ScraperError` variant via `From` for backward compatibility.
#[derive(Error, Debug)]
pub enum DomainError {
    /// URL is invalid or cannot be parsed
    #[error("URL inválida: {0}")]
    InvalidUrl(String),

    /// Content extraction failed (e.g., Readability algorithm failure)
    #[error("Error de legibilidad: {0}")]
    Readability(String),

    /// Asset extraction failed
    #[error("Error de extracción: {0}")]
    Extraction(String),

    /// Content extraction failed (poor fallback content)
    #[error("extracción falló para {url}: {reason}")]
    ExtractionFailed {
        /// URL that failed
        url: String,
        /// Reason for failure
        reason: String,
    },

    /// URL validation failed
    #[error("Validación de URL falló: {0}")]
    Validation(String),

    /// Feature not yet implemented (gated for future release)
    #[error("funcionalidad no disponible: {0}")]
    FeatureGated(String),

    /// Conversion error (HTML to Markdown, YAML, etc.)
    #[error("Error de conversión: {0}")]
    Conversion(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_error_invalid_url() {
        let err = DomainError::InvalidUrl("URL vacía".to_string());
        assert_eq!(err.to_string(), "URL inválida: URL vacía");
    }

    #[test]
    fn test_domain_error_extraction_failed() {
        let err = DomainError::ExtractionFailed {
            url: "https://example.com".to_string(),
            reason: "no content".to_string(),
        };
        assert!(err.to_string().contains("example.com"));
        assert!(err.to_string().contains("no content"));
    }

    #[test]
    fn test_domain_error_is_std_error() {
        let err = DomainError::Validation("invalid".to_string());
        let _: &dyn std::error::Error = &err;
    }
}
