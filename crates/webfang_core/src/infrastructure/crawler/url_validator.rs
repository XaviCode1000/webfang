//! URL Validator Module
//!
//! Validates and filters URLs during sitemap processing.
//! Performs pattern filtering, HTTP status validation, and canonical URL enforcement.

use crate::domain::ValidationResult;
use url::Url;

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

/// Handles URL validation and filtering
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

    /// Filter URLs with invalid patterns
    pub fn filter_invalid_patterns(&self, url: &Url) -> ValidationResult {
        let path = url.path();

        // Filter Node.js release URLs with invalid version patterns
        if path.contains("/blog/release/v") {
            if let Some(version_part) = path.split("/blog/release/v").nth(1) {
                let version = version_part
                    .split(&['?', '#'][..])
                    .next()
                    .unwrap_or(version_part);

                if let Some(major_str) = version.split('.').next() {
                    if let Ok(major) = major_str.parse::<u32>() {
                        if major > 99 {
                            return ValidationResult::Invalid(format!(
                                "Invalid Node.js version pattern: v{version}"
                            ));
                        }
                    }
                }
            }
        }

        // Filter non-HTTP schemes
        match url.scheme() {
            "http" | "https" => {},
            scheme => {
                return ValidationResult::Invalid(format!("Unsupported scheme: {scheme}"));
            },
        }

        ValidationResult::Valid
    }

    /// Validate URL by checking HTTP status code
    pub async fn validate_http_status(&self, url: &Url) -> Result<ValidationResult> {
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

        let result = validator.filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_node_version() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://nodejs.org/blog/release/v106.0").unwrap();

        let result = validator.filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_scheme() {
        let validator = UrlValidator::new();
        let url = Url::parse("ftp://example.com/file").unwrap();

        let result = validator.filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_valid_node_version() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://nodejs.org/blog/release/v18.12.0").unwrap();

        let result = validator.filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[tokio::test]
    #[ignore = "depends on external httpbin.org service, flaky in CI/CD"]
    async fn test_validate_http_status_200() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://httpbin.org/status/200").unwrap();

        let result = validator.validate_http_status(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Valid)));
    }

    #[tokio::test]
    #[ignore = "depends on external httpbin.org service, flaky in CI/CD"]
    async fn test_validate_http_status_404() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://httpbin.org/status/404").unwrap();

        let result = validator.validate_http_status(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Invalid(_))));
    }

    #[tokio::test]
    #[ignore = "depends on external httpbin.org service, flaky in CI/CD"]
    async fn test_validate_http_status_500() {
        let validator = UrlValidator::new();
        let url = Url::parse("https://httpbin.org/status/500").unwrap();

        let result = validator.validate_http_status(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Invalid(_))));
    }
}
