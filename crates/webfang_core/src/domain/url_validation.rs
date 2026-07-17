//! URL validation utilities.

use crate::error::ScraperError;
use crate::Result;

/// Validate and parse a URL string using the `url` crate (RFC 3986 compliant).
///
/// This function performs strict URL validation:
/// - Trims whitespace automatically
/// - Requires http or https scheme (case-insensitive)
/// - Requires a valid host
/// - Rejects malformed URLs
///
/// # Arguments
///
/// * `url` - URL string to validate and parse
///
/// # Returns
///
/// * `Ok(url::Url)` - Validated and parsed URL
/// * `Err(ScraperError::InvalidUrl)` - Invalid URL with error message
///
/// # Errors
///
/// Returns an error if:
/// - URL is empty
/// - URL has invalid format
/// - URL scheme is not http or https
/// - URL has no host
///
/// # Examples
///
/// ```
/// use webfang::validate_and_parse_url;
///
/// // Valid URLs
/// let url = validate_and_parse_url("https://example.com").unwrap();
/// assert_eq!(url.host_str(), Some("example.com"));
///
/// let url = validate_and_parse_url("HTTP://EXAMPLE.COM").unwrap();
/// assert_eq!(url.scheme(), "http");
///
/// // Invalid URLs
/// assert!(validate_and_parse_url("").is_err());
/// assert!(validate_and_parse_url("ftp://example.com").is_err());
/// assert!(validate_and_parse_url("not-a-url").is_err());
/// ```
///
/// # Whitespace Handling
///
/// Leading and trailing whitespace is automatically trimmed:
///
/// ```
/// use webfang::validate_and_parse_url;
///
/// let url = validate_and_parse_url("  https://example.com  ").unwrap();
/// assert_eq!(url.host_str(), Some("example.com"));
/// ```
pub fn validate_and_parse_url(url: &str) -> Result<url::Url> {
    if url.is_empty() {
        return Err(ScraperError::invalid_url("URL cannot be empty"));
    }

    let parsed = url::Url::parse(url.trim())
        .map_err(|e| ScraperError::invalid_url(format!("Failed to parse URL '{url}': {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {},
        scheme => {
            return Err(ScraperError::invalid_url(format!(
                "URL must use http or https scheme, got '{scheme}'"
            )))
        },
    }

    if parsed.host_str().is_none() {
        return Err(ScraperError::invalid_url("URL must have a valid host"));
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_and_parse_url_success() {
        let url = validate_and_parse_url("https://example.com");
        assert!(url.is_ok());
    }

    #[test]
    fn test_validate_and_parse_url_empty() {
        let result = validate_and_parse_url("");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_and_parse_url_invalid_scheme() {
        let result = validate_and_parse_url("ftp://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_and_parse_url_whitespace() {
        let url = validate_and_parse_url("  https://example.com  ");
        assert!(url.is_ok());
        assert_eq!(url.unwrap().host_str(), Some("example.com"));
    }
}
