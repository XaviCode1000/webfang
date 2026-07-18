//! URL Validator Module
//!
//! Validates and filters URLs during sitemap processing.
//! Performs pattern filtering, HTTP status validation, and canonical URL enforcement.
//!
//! This module implements the domain `UrlValidatorTrait` for HTTP-aware validation.

use url::Url;

use crate::domain::{DomainError, UrlValidatorTrait, ValidationResult};

/// Errors that can occur during URL validation
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("HTTP request failed: {0}")]
    HttpError(String),
    #[error("URL validation timeout")]
    Timeout,
}

/// Result type for validation operations
pub type Result<T> = std::result::Result<T, ValidationError>;

impl From<ValidationError> for DomainError {
    fn from(err: ValidationError) -> Self {
        DomainError::Validation(err.to_string())
    }
}

/// Handles URL validation and filtering
///
/// HTTP-aware implementation of `UrlValidatorTrait`.
/// Delegates pattern filtering to the domain's `StaticUrlValidator`
/// and adds HTTP status validation.
pub struct UrlValidator {
    client: wreq::Client,
    #[allow(dead_code)]
    timeout_ms: u64,
}

impl UrlValidator {
    /// Create new URL validator with default settings
    pub fn new() -> Self {
        Self {
            client: wreq::Client::builder()
                .emulation(wreq_util::Emulation::Chrome145)
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("BUG: failed to build HTTP client"),
            timeout_ms: 10_000,
        }
    }

    /// Create validator with custom timeout
    pub fn with_timeout(timeout_ms: u64) -> Self {
        Self {
            timeout_ms,
            ..Self::new()
        }
    }

    /// Validate URL by checking HTTP status code
    ///
    /// This is the infra-specific method that makes actual HTTP calls.
    /// The `UrlValidatorTrait::validate_http_status` default returns `Ok(Valid)`;
    /// this impl overrides it with real HTTP behavior.
    async fn validate_http_status_inner(&self, url: &Url) -> Result<ValidationResult> {
        let response = self
            .client
            .head(url.as_str())
            .send()
            .await
            .map_err(|e| ValidationError::HttpError(e.to_string()))?;

        let status = response.status().as_u16();

        match status {
            200..=299 => Ok(ValidationResult::Valid),
            301 | 302 | 307 | 308 => {
                // Follow redirect
                if let Some(location) = response.headers().get("location") {
                    if let Ok(location_str) = location.to_str() {
                        if let Ok(new_url) = Url::parse(location_str) {
                            return Ok(ValidationResult::NeedsRedirect(new_url));
                        }
                    }
                }
                Ok(ValidationResult::Valid) // Treat redirect as valid if we can't follow
            },
            404 | 410 => Ok(ValidationResult::Invalid(format!(
                "URL not found (status {status})"
            ))),
            _ => Ok(ValidationResult::Invalid(format!(
                "HTTP error (status {status})"
            ))),
        }
    }
}

impl Default for UrlValidator {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlValidatorTrait for UrlValidator {
    /// Delegates pattern filtering to the domain's pure logic
    fn filter_invalid_patterns(&self, url: &Url) -> ValidationResult {
        crate::domain::StaticUrlValidator::filter_invalid_patterns(url)
    }

    /// Real HTTP status validation via `wreq`
    async fn validate_http_status(
        &self,
        url: &Url,
    ) -> std::result::Result<ValidationResult, DomainError> {
        self.validate_http_status_inner(url)
            .await
            .map_err(DomainError::from)
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_url_validator_creation() {
        let validator = UrlValidator::new();
        // Just test that it can be created without panicking
        let _ = validator;
    }

    #[test]
    fn test_filter_invalid_patterns_valid_url() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://example.com/page").unwrap();

        // Uses trait method
        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_node_version() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://nodejs.org/blog/release/v106.0").unwrap();

        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_scheme() {
        let validator = UrlValidator::new();
        let url = Url::parse("ftp://example.com/file").unwrap();

        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_valid_node_version() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://nodejs.org/blog/release/v18.12.0").unwrap();

        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_filter_invalid_patterns_delegates_to_domain() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://example.com/page").unwrap();

        let from_infra = validator.filter_invalid_patterns(&url);
        let from_domain = crate::domain::StaticUrlValidator::filter_invalid_patterns(&url);
        assert_eq!(from_infra, from_domain);
    }

    #[tokio::test]
    #[ignore = "depends on external httpbin.org service, flaky in CI/CD"]
    async fn test_validate_http_status_200() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://httpbin.org/status/200").unwrap();

        let result = validator.validate_http_status_inner(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Valid)));
    }

    #[tokio::test]
    #[ignore = "depends on external httpbin.org service, flaky in CI/CD"]
    async fn test_validate_http_status_404() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://httpbin.org/status/404").unwrap();

        let result = validator.validate_http_status_inner(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Invalid(_))));
    }

    #[tokio::test]
    #[ignore = "depends on external httpbin.org service, flaky in CI/CD"]
    async fn test_validate_http_status_500() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://httpbin.org/status/500").unwrap();

        let result = validator.validate_http_status_inner(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Invalid(_))));
    }
}
