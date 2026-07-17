//! Domain URL validator service
//!
//! Pure business logic for URL validation without external dependencies.
//!
//! This service contains the core validation rules that don't require
//! network calls or external libraries.

use crate::domain::ValidationResult;

/// Domain service for URL validation logic
///
/// Contains pure functions for validating URLs according to business rules.
/// No external dependencies - can be used in tests without mocking.
pub struct UrlValidator;

impl UrlValidator {
    /// Filter URLs with invalid patterns
    ///
    /// Validates URLs against business rules like supported schemes
    /// and invalid version patterns (e.g., Node.js release URLs).
    pub fn filter_invalid_patterns(url: &url::Url) -> ValidationResult {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_invalid_patterns_valid_url() {
        let url = url::Url::parse("https://example.com/page").unwrap();
        let result = UrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Valid));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_node_version() {
        let url = url::Url::parse("https://nodejs.org/blog/release/v106.0").unwrap();
        let result = UrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_invalid_scheme() {
        let url = url::Url::parse("ftp://example.com/file").unwrap();
        let result = UrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Invalid(_)));
    }

    #[test]
    fn test_filter_invalid_patterns_valid_node_version() {
        let url = url::Url::parse("https://nodejs.org/blog/release/v18.12.0").unwrap();
        let result = UrlValidator::filter_invalid_patterns(&url);
        assert!(matches!(result, ValidationResult::Valid));
    }
}
