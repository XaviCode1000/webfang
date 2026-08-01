//! URL Validator Module
//!
//! Validates and filters URLs during sitemap processing.
//! Performs pattern filtering, HTTP status validation, and canonical URL enforcement.
//!
//! This module implements the domain `UrlValidatorTrait` for HTTP-aware validation.

use url::Url;

use crate::domain::{CrawlError, DomainError, UrlValidatorTrait, ValidationResult};

/// Errors that can occur during URL validation
#[derive(Debug, thiserror::Error)]
pub(crate) enum ValidationError {
    #[error("HTTP request failed: {0}")]
    HttpError(String),
    #[error("URL validation timeout")]
    #[allow(dead_code)] // pub(crate) Phase 0 triage — internal API surface
    Timeout,
}

/// Result type for validation operations
pub(crate) type Result<T> = std::result::Result<T, ValidationError>;

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
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`UrlValidator::with_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the underlying HTTP client fails to build.
    pub fn new() -> std::result::Result<Self, CrawlError> {
        Self::with_profile(wreq_util::Profile::Chrome145)
    }

    /// Create new URL validator with an explicit TLS/H2 profile.
    ///
    /// The client is built via the shared `create_http_client_with_config`
    /// factory (#299), honoring the caller's `--h2-profile` selection instead
    /// of a hardcoded preset (#324).
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the underlying HTTP client fails to build.
    pub fn with_profile(
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<Self, CrawlError> {
        Self::with_timeout_and_profile(10_000, tls_emulation)
    }

    /// Create validator with custom timeout
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`UrlValidator::with_timeout_and_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the underlying HTTP client fails to build.
    pub fn with_timeout(timeout_ms: u64) -> std::result::Result<Self, CrawlError> {
        Self::with_timeout_and_profile(timeout_ms, wreq_util::Profile::Chrome145)
    }

    /// Create validator with custom timeout and an explicit TLS/H2 profile.
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the underlying HTTP client fails to build.
    pub fn with_timeout_and_profile(
        timeout_ms: u64,
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<Self, CrawlError> {
        let client = Self::build_client(tls_emulation)?;
        Ok(Self { client, timeout_ms })
    }

    /// Build the validation HTTP client for a given TLS/H2 profile.
    ///
    /// Request and connect timeouts are pinned to 10s to preserve the
    /// historical behavior of the previous hardcoded client (#324).
    fn build_client(
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<wreq::Client, CrawlError> {
        let http_config = crate::domain::http_config::HttpClientConfig {
            tls_emulation,
            timeout_secs: 10,
            connect_timeout_secs: 10,
            ..Default::default()
        };
        crate::infrastructure::http::create_http_client_with_config(&http_config)
            .map_err(|e| CrawlError::Internal(format!("Failed to create HTTP client: {e}")))
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
        assert!(UrlValidator::new().is_ok());
    }

    #[test]
    fn test_url_validator_with_custom_profile() {
        assert!(UrlValidator::with_profile(wreq_util::Profile::Chrome131).is_ok());
        assert!(UrlValidator::with_profile(wreq_util::Profile::Firefox135).is_ok());
    }

    #[test]
    fn test_url_validator_with_timeout_and_profile() {
        assert!(
            UrlValidator::with_timeout_and_profile(5_000, wreq_util::Profile::Chrome131).is_ok()
        );
    }

    #[test]
    fn test_filter_invalid_patterns_valid_url() {
        let validator = UrlValidator::new().unwrap();
        let url = Url::parse("https://example.com/page").unwrap();

        // Uses trait method
        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_node_version() {
        let validator = UrlValidator::new().unwrap();
        let url = Url::parse("https://nodejs.org/blog/release/v106.0").unwrap();

        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_scheme() {
        let validator = UrlValidator::new().unwrap();
        let url = Url::parse("ftp://example.com/file").unwrap();

        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_valid_node_version() {
        let validator = UrlValidator::new().unwrap();
        let url = Url::parse("https://nodejs.org/blog/release/v18.12.0").unwrap();

        let result = <UrlValidator as UrlValidatorTrait>::filter_invalid_patterns(&validator, &url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_filter_invalid_patterns_delegates_to_domain() {
        let validator = UrlValidator::new().unwrap();
        let url = Url::parse("https://example.com/page").unwrap();

        let from_infra = validator.filter_invalid_patterns(&url);
        let from_domain = crate::domain::StaticUrlValidator::filter_invalid_patterns(&url);
        assert_eq!(from_infra, from_domain);
    }

    #[tokio::test]
    async fn test_validate_http_status_200() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let validator = UrlValidator::new().unwrap();
        let url = Url::parse(&server.uri()).unwrap();

        let result = validator.validate_http_status_inner(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Valid)));
    }

    #[tokio::test]
    async fn test_validate_http_status_404() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let validator = UrlValidator::new().unwrap();
        let url = Url::parse(&server.uri()).unwrap();

        let result = validator.validate_http_status_inner(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Invalid(_))));
    }

    #[tokio::test]
    async fn test_validate_http_status_500() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("HEAD"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let validator = UrlValidator::new().unwrap();
        let url = Url::parse(&server.uri()).unwrap();

        let result = validator.validate_http_status_inner(&url).await;
        assert!(matches!(result, Ok(ValidationResult::Invalid(_))));
    }
}
