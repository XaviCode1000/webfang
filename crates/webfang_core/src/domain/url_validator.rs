//! Domain URL validator service
//!
//! Pure business logic for URL validation without external dependencies.
//!
//! This module defines a `UrlValidatorTrait` trait and a `StaticUrlValidator`
//! implementation with the core validation rules that don't require
//! network calls or external libraries.

use std::future::Future;

use url::Url;

use crate::domain::{DomainError, ValidationResult};

/// Trait for URL validation logic.
///
/// Domain defines the contract; infrastructure provides HTTP-aware
/// implementations while the domain itself provides a pure, stateless
/// implementation (`StaticUrlValidator`) suitable for tests.
///
/// Uses native `impl Future` return position in trait (Rust 1.88+),
/// no `async-trait` crate needed.
pub trait UrlValidatorTrait: Send + Sync {
    /// Filter URLs with invalid patterns (Node.js version, unsupported schemes)
    fn filter_invalid_patterns(&self, url: &Url) -> ValidationResult;

    /// Validate URL by checking HTTP status code.
    ///
    /// Default implementation returns `Valid` — pure/static validators
    /// don't perform HTTP checks.
    fn validate_http_status(
        &self,
        _url: &Url,
    ) -> impl Future<Output = Result<ValidationResult, DomainError>> + Send {
        std::future::ready(Ok(ValidationResult::Valid))
    }
}

/// Pure, stateless implementation of `UrlValidatorTrait`.
///
/// Contains the core business rules that don't require HTTP calls.
/// Suitable for tests and non-network validation scenarios.
pub struct StaticUrlValidator;

impl StaticUrlValidator {
    /// Filter URLs with invalid patterns (associated function for direct use)
    ///
    /// Validates URLs against business rules like supported schemes
    /// and invalid version patterns (e.g., Node.js release URLs).
    pub fn filter_invalid_patterns(url: &Url) -> ValidationResult {
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
}

impl UrlValidatorTrait for StaticUrlValidator {
    fn filter_invalid_patterns(&self, url: &Url) -> ValidationResult {
        Self::filter_invalid_patterns(url)
    }
}

/// Re-export the old name for backward compatibility
pub type UrlValidator = StaticUrlValidator;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_invalid_patterns_valid_url() {
        let url = Url::parse("https://example.com/page").unwrap();
        let result = StaticUrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_node_version() {
        let url = Url::parse("https://nodejs.org/blog/release/v106.0").unwrap();
        let result = StaticUrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_scheme() {
        let url = Url::parse("ftp://example.com/file").unwrap();
        let result = StaticUrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_valid_node_version() {
        let url = Url::parse("https://nodejs.org/blog/release/v18.12.0").unwrap();
        let result = StaticUrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_trait_impl_delegates_to_static() {
        let validator = StaticUrlValidator;
        let url = Url::parse("https://example.com/page").unwrap();
        let result = validator.filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Valid));
    }
}
