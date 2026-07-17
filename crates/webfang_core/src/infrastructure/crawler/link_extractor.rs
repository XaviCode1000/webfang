//! Link extraction from HTML
//!
//! Extracts and normalizes links from HTML content.
//!
//! # Rules Applied
//!
//! - **own-borrow-over-clone**: Accept `&str` not `&String`
//! - **opt-inline**: Inline hot path functions
//! - **mem-with-capacity**: Pre-allocate Vec when size is estimable

use scraper::{Html, Selector};
use tracing::debug;
use url::Url;

use crate::domain::LinkExtractor;

/// Extract all links from HTML content
///
/// Following **own-borrow-over-clone**: Accepts `&str` not `&String`.
/// Following **mem-with-capacity**: Pre-allocates Vec with estimated capacity.
///
/// # Arguments
///
/// * `html` - HTML content to parse
/// * `base_url` - Base URL for resolving relative links
///
/// # Returns
///
/// * `Ok(Vec<String>)` - List of extracted URLs
/// * `Err(CrawlError)` - Parse error
///
/// # Examples
///
/// ```
/// use webfang::infrastructure::crawler::extract_links;
///
/// let html = r#"<html><body><a href="/page1">Link 1</a><a href="https://other.com/page2">Link 2</a></body></html>"#;
/// let links = extract_links(html, "https://example.com").unwrap();
/// assert!(links.contains(&"https://example.com/page1".to_string()));
/// assert!(links.contains(&"https://other.com/page2".to_string()));
/// ```
pub fn extract_links(html: &str, base_url: &str) -> Result<Vec<String>, crate::domain::CrawlError> {
    debug!("Extracting links from HTML (base_url={})", base_url);

    let document = Html::parse_document(html);
    let selector = Selector::parse("a[href]")
        .map_err(|e| crate::domain::CrawlError::Parse(format!("Failed to parse selector: {e}")))?;

    // Parse base URL once
    let base =
        Url::parse(base_url).map_err(|e| crate::domain::CrawlError::InvalidUrl(e.to_string()))?;

    // Pre-allocate with estimated capacity (optimization for typical pages)
    let mut links = Vec::with_capacity(32);

    for element in document.select(&selector) {
        if let Some(href) = element.value().attr("href") {
            // Resolve relative URLs
            match base.join(href) {
                Ok(absolute_url) => {
                    let normalized = normalize_url(absolute_url.as_str());
                    if !links.contains(&normalized) {
                        links.push(normalized);
                    }
                },
                Err(e) => {
                    debug!("Failed to resolve URL '{}': {}", href, e);
                },
            }
        }
    }

    debug!("Extracted {} links from {}", links.len(), base_url);
    Ok(links)
}

/// Check if a URL is internal (same domain)
///
/// Following **own-borrow-over-clone**: Accepts `&str` for both parameters.
/// Following **opt-inline**: Inlined for hot path performance.
///
/// # Arguments
///
/// * `url` - URL to check
/// * `domain` - Domain to check against
///
/// # Returns
///
/// `true` if the URL belongs to the domain
///
/// # Examples
///
/// ```
/// use webfang::infrastructure::crawler::is_internal_link;
///
/// assert!(is_internal_link("https://example.com/page", "example.com"));
/// assert!(is_internal_link("https://www.example.com/page", "example.com"));
/// assert!(!is_internal_link("https://other.com/page", "example.com"));
/// ```
#[inline]
#[must_use]
pub fn is_internal_link(url: &str, domain: &str) -> bool {
    extract_domain(url)
        .map(|url_domain| url_domain == domain || url_domain.ends_with(&format!(".{domain}")))
        .unwrap_or(false)
}

/// Normalize a URL (remove fragments, strip www, remove default ports, etc.)
///
/// Following **own-borrow-over-clone**: Accepts `&str`.
///
/// This is the **canonical** URL normalizer for the scraper. All URL
/// normalization should go through this function.
///
/// Options:
/// - `strip_hash: true` — removes URL fragments (`#section`)
/// - `strip_www: true` — removes `www.` prefix for better dedup
/// - `remove_trailing_slash: false` — preserves trailing slashes
/// - `remove_query_parameters: All` — strips query strings for dedup
/// - `sort_query_parameters: true` — consistent ordering
///
/// # Arguments
///
/// * `url` - URL to normalize
///
/// # Returns
///
/// Normalized URL string
///
/// # Examples
///
/// ```
/// use webfang::infrastructure::crawler::normalize_url;
///
/// assert_eq!(
///     normalize_url("https://example.com/page#section"),
///     "https://example.com/page"
/// );
/// assert_eq!(
///     normalize_url("https://www.example.com/page"),
///     "https://example.com/page"
/// );
/// assert_eq!(
///     normalize_url("https://example.com:443/page"),
///     "https://example.com/page"
/// );
/// ```
#[inline]
#[must_use]
pub fn normalize_url(url: &str) -> String {
    use url_normalize::{normalize_url as normalize, Options, RemoveQueryParameters};

    // Non-URLs (no scheme) should not be normalized — return as-is.
    // This prevents "not-a-valid-url" → "http://not-a-valid-url" conversion.
    if !url.contains("://") {
        return url.to_string();
    }

    let opts = Options {
        strip_hash: true,
        remove_trailing_slash: false,
        remove_query_parameters: RemoveQueryParameters::All,
        sort_query_parameters: true,
        strip_www: true,
        force_https: false,
        ..Options::default()
    };

    // url-normalize handles WHATWG preprocessing (control chars, backslashes,
    // trailing whitespace) and produces idempotent output.
    normalize(url, &opts).unwrap_or_else(|_| url.to_string())
}

/// Extract domain from URL
///
/// Following **own-borrow-over-clone**: Accepts `&str`.
/// Following **opt-inline**: Inlined for hot path.
///
/// # Arguments
///
/// * `url` - URL to extract domain from
///
/// # Returns
///
/// Domain string or None if invalid
#[inline]
#[must_use]
fn extract_domain(url: &str) -> Option<&str> {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
}

/// HTML link extractor implementation
///
/// Implements the domain LinkExtractor trait using scraper library.
pub struct HtmlLinkExtractor;

impl LinkExtractor for HtmlLinkExtractor {
    fn extract_links(
        &self,
        html: &str,
        base_url: &str,
    ) -> Result<Vec<String>, crate::domain::CrawlError> {
        extract_links(html, base_url)
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_basic() {
        let html = r#"
            <html>
                <body>
                    <a href="/page1">Link 1</a>
                    <a href="/page2">Link 2</a>
                    <a href="https://other.com/external">External</a>
                </body>
            </html>
        "#;

        let links = extract_links(html, "https://example.com").unwrap();

        assert!(links.contains(&"https://example.com/page1".to_string()));
        assert!(links.contains(&"https://example.com/page2".to_string()));
        assert!(links.contains(&"https://other.com/external".to_string()));
        assert_eq!(links.len(), 3);
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_relative_paths() {
        let html = r#"
            <html>
                <body>
                    <a href="../parent">Parent</a>
                    <a href="./current">Current</a>
                    <a href="sub/child">Child</a>
                </body>
            </html>
        "#;

        let links = extract_links(html, "https://example.com/dir/page").unwrap();

        assert!(links.contains(&"https://example.com/parent".to_string()));
        assert!(links.contains(&"https://example.com/dir/current".to_string()));
        assert!(links.contains(&"https://example.com/dir/sub/child".to_string()));
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_no_duplicates() {
        let html = r#"
            <html>
                <body>
                    <a href="/page">Link 1</a>
                    <a href="/page">Link 2</a>
                    <a href="/page">Link 3</a>
                </body>
            </html>
        "#;

        let links = extract_links(html, "https://example.com").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "https://example.com/page");
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_empty() {
        let html = r#"<html><body>No links here</body></html>"#;
        let links = extract_links(html, "https://example.com").unwrap();
        assert!(links.is_empty());
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_invalid_html() {
        let html = "This is not HTML at all";
        let links = extract_links(html, "https://example.com").unwrap();
        assert!(links.is_empty());
    }

    #[test]
    fn test_is_internal_link() {
        assert!(is_internal_link("https://example.com/page", "example.com"));
        assert!(is_internal_link(
            "https://www.example.com/page",
            "example.com"
        ));
        assert!(is_internal_link(
            "https://blog.example.com/post",
            "example.com"
        ));
        assert!(!is_internal_link("https://other.com/page", "example.com"));
        assert!(!is_internal_link("invalid-url", "example.com"));
    }

    #[test]
    fn test_normalize_url_remove_fragment() {
        assert_eq!(
            normalize_url("https://example.com/page#section"),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_url("https://example.com/page#top"),
            "https://example.com/page"
        );
    }

    #[test]
    fn test_normalize_url_preserve_trailing_slash() {
        assert_eq!(
            normalize_url("https://example.com/page/"),
            "https://example.com/page/"
        );
        assert_eq!(
            normalize_url("https://example.com/page/#section"),
            "https://example.com/page/"
        );
    }

    #[test]
    fn test_normalize_url_no_change() {
        assert_eq!(
            normalize_url("https://example.com/page"),
            "https://example.com/page"
        );
    }

    #[test]
    fn test_normalize_url_invalid() {
        let result = normalize_url("not-a-valid-url");
        assert_eq!(result, "not-a-valid-url");
    }

    #[test]
    fn test_normalize_url_strips_www() {
        assert_eq!(
            normalize_url("https://www.example.com/page"),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_url("https://www.example.com/page/"),
            "https://example.com/page/"
        );
    }

    #[test]
    fn test_normalize_url_removes_default_port() {
        assert_eq!(
            normalize_url("https://example.com:443/page"),
            "https://example.com/page"
        );
        assert_eq!(
            normalize_url("http://example.com:80/page"),
            "http://example.com/page"
        );
    }

    // ============================================================================
    // Error path tests
    // ============================================================================

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_javascript_mailto_included() {
        // extract_links does NOT filter javascript:/mailto:/tel: schemes
        // it resolves them via base.join() which includes them
        let html = r#"
            <html>
                <body>
                    <a href="/valid">Valid Link</a>
                    <a href="javascript:alert(1)">JavaScript</a>
                    <a href="mailto:test@example.com">Email</a>
                    <a href="tel:+1234567890">Phone</a>
                </body>
            </html>
        "#;

        let links = extract_links(html, "https://example.com").unwrap();
        // All links are included (no filtering of special schemes)
        assert_eq!(links.len(), 4);
        assert!(links.contains(&"https://example.com/valid".to_string()));
        // javascript:, mailto:, tel: are resolved relative to base
        assert!(links.iter().any(|l| l.contains("javascript")));
        assert!(links.iter().any(|l| l.contains("mailto")));
        assert!(links.iter().any(|l| l.contains("tel")));
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_empty_href() {
        let html = r#"
            <html>
                <body>
                    <a href="">Empty href</a>
                    <a href="/page">Valid link</a>
                </body>
            </html>
        "#;

        let links = extract_links(html, "https://example.com").unwrap();
        // Empty href resolves to the base URL itself (no trailing slash added)
        assert!(links.contains(&"https://example.com".to_string()));
        assert!(links.contains(&"https://example.com/page".to_string()));
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_links_with_query_params() {
        // Note: normalize_url keeps path but strips fragments for dedup
        // Query params in href are resolved but may be normalized
        let html = r#"
            <html>
                <body>
                    <a href="/search?q=rust&lang=en">Search</a>
                    <a href="/page?foo=bar#section">With fragment</a>
                </body>
            </html>
        "#;

        let links = extract_links(html, "https://example.com").unwrap();
        assert_eq!(links.len(), 2);
        // Links contain the path portion; query params may be normalized
        assert!(links.iter().any(|l| l.contains("/search")));
        assert!(links.iter().any(|l| l.contains("/page")));
    }
}
