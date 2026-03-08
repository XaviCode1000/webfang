//! Value objects — Type-safe primitives
//!
//! Value objects are immutable types that are defined by their attributes,
//! not by identity. They provide type safety at compile time.

use serde::{Deserialize, Serialize};

/// Validated URL newtype - guarantees URL is valid at type level
///
/// This enforces that ScrapedContent always has a valid URL,
/// preventing runtime errors from invalid URLs.
///
/// # Examples
///
/// ```
/// use rust_scraper::domain::ValidUrl;
///
/// // Create from parsed URL
/// let url = url::Url::parse("https://example.com").unwrap();
/// let valid = ValidUrl::new(url);
/// assert_eq!(valid.as_str(), "https://example.com/");  // URL adds trailing slash
///
/// // Or parse directly
/// let valid = ValidUrl::parse("https://example.com").unwrap();
/// assert!(valid.as_str().starts_with("https://example.com"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidUrl(url::Url);

impl ValidUrl {
    /// Create a new ValidUrl from a validated url::Url
    ///
    /// This is infallible since the URL is already parsed.
    pub fn new(url: url::Url) -> Self {
        Self(url)
    }

    /// Parse and create a ValidUrl from a string
    ///
    /// Returns error if the string is not a valid URL.
    ///
    /// # Examples
    ///
    /// ```
    /// use rust_scraper::domain::ValidUrl;
    ///
    /// let url = ValidUrl::parse("https://example.com").unwrap();
    /// assert_eq!(url.host_str(), Some("example.com"));
    ///
    /// let invalid = ValidUrl::parse("not-a-url");
    /// assert!(invalid.is_err());
    /// ```
    pub fn parse(s: &str) -> crate::Result<Self> {
        Ok(Self(url::Url::parse(s).map_err(|e| {
            crate::ScraperError::invalid_url(e.to_string())
        })?))
    }

    /// Get reference to inner url::Url
    pub fn as_url(&self) -> &url::Url {
        &self.0
    }

    /// Get the URL as string
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Get the host portion of the URL
    pub fn host_str(&self) -> Option<&str> {
        self.0.host_str()
    }

    /// Get the scheme (protocol) of the URL
    pub fn scheme(&self) -> &str {
        self.0.scheme()
    }

    /// Get the path portion of the URL
    pub fn path(&self) -> &str {
        self.0.path()
    }
}

impl From<url::Url> for ValidUrl {
    fn from(url: url::Url) -> Self {
        Self(url)
    }
}

impl std::fmt::Display for ValidUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_url_new() {
        let url = url::Url::parse("https://example.com").unwrap();
        let valid = ValidUrl::new(url);
        assert_eq!(valid.as_str(), "https://example.com/"); // URL adds trailing slash
    }

    #[test]
    fn test_valid_url_parse_success() {
        let valid = ValidUrl::parse("https://example.com/article");
        assert!(valid.is_ok());
        let valid = valid.unwrap();
        assert_eq!(valid.host_str(), Some("example.com"));
        assert_eq!(valid.path(), "/article");
    }

    #[test]
    fn test_valid_url_parse_invalid() {
        let result = ValidUrl::parse("not-a-url");
        assert!(result.is_err());
    }

    #[test]
    fn test_valid_url_from_trait() {
        let url = url::Url::parse("https://example.com").unwrap();
        let valid: ValidUrl = url.into();
        assert_eq!(valid.as_str(), "https://example.com/"); // URL adds trailing slash
    }

    #[test]
    fn test_valid_url_display() {
        let url = ValidUrl::parse("https://example.com").unwrap();
        assert_eq!(format!("{}", url), "https://example.com/");
    }

    #[test]
    fn test_valid_url_with_query() {
        let url = ValidUrl::parse("https://example.com/search?q=rust").unwrap();
        assert_eq!(url.as_url().query(), Some("q=rust"));
    }

    #[test]
    fn test_valid_url_with_port() {
        let url = ValidUrl::parse("http://localhost:8080/api").unwrap();
        assert_eq!(url.host_str(), Some("localhost"));
        assert_eq!(url.as_url().port(), Some(8080));
    }
}
