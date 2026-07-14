//! Crawl job entities
//!
//! Core entities representing URLs discovered during crawling.

use url::Url;

/// Content type discovered during crawling
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ContentType {
    /// HTML page
    #[default]
    Html,
    /// XML document (including sitemaps)
    Xml,
    /// Plain text
    Text,
    /// Unknown or other content type
    Other,
}

/// A discovered URL during crawling
///
/// Note: Cannot derive `Copy` because `Url` is not `Copy`.
/// Following **own-borrow-over-clone**: We'll pass references where possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredUrl {
    /// The discovered URL
    pub url: Url,
    /// Depth in the crawl tree (0 = seed)
    pub depth: u8,
    /// Parent URL that led to this discovery
    pub parent_url: Url,
    /// Content type if known
    pub content_type: ContentType,
}

impl DiscoveredUrl {
    /// Create a new discovered URL
    #[must_use]
    pub fn new(url: Url, depth: u8, parent_url: Url, content_type: ContentType) -> Self {
        Self {
            url,
            depth,
            parent_url,
            content_type,
        }
    }

    /// Create a new discovered URL with default HTML content type
    #[must_use]
    pub fn html(url: Url, depth: u8, parent_url: Url) -> Self {
        Self {
            url,
            depth,
            parent_url,
            content_type: ContentType::Html,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discovered_url_dedup_via_url_deduplicator() {
        use crate::application::deduplicator::UrlDeduplicator;

        let dedup = UrlDeduplicator::new();
        assert!(dedup.try_insert("https://example.com/page"));
        assert!(!dedup.try_insert("https://example.com/page")); // duplicate
        assert!(dedup.try_insert("https://example.com/other")); // different URL
        assert_eq!(dedup.len(), 2);
    }

    #[test]
    fn test_discovered_url_new() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::new(url, 1, parent, ContentType::Html);

        assert_eq!(discovered.depth, 1);
        assert_eq!(discovered.content_type, ContentType::Html);
    }

    #[test]
    fn test_discovered_url_html() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::html(url, 0, parent);

        assert_eq!(discovered.depth, 0);
        assert_eq!(discovered.content_type, ContentType::Html);
    }

    // -- ContentType tests --

    #[test]
    fn content_type_default_is_html() {
        assert_eq!(ContentType::default(), ContentType::Html);
    }

    #[test]
    fn content_type_debug_all_variants() {
        assert_eq!(format!("{:?}", ContentType::Html), "Html");
        assert_eq!(format!("{:?}", ContentType::Xml), "Xml");
        assert_eq!(format!("{:?}", ContentType::Text), "Text");
        assert_eq!(format!("{:?}", ContentType::Other), "Other");
    }

    #[test]
    fn content_type_clone() {
        let ct = ContentType::Xml;
        let cloned = ct;
        assert_eq!(ct, cloned);
    }

    #[test]
    fn content_type_equality() {
        assert_eq!(ContentType::Html, ContentType::Html);
        assert_ne!(ContentType::Html, ContentType::Xml);
        assert_ne!(ContentType::Text, ContentType::Other);
    }

    // -- DiscoveredUrl edge cases --

    #[test]
    fn discovered_url_equality_same_values() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();

        let a = DiscoveredUrl::new(url.clone(), 1, parent.clone(), ContentType::Html);
        let b = DiscoveredUrl::new(url, 1, parent, ContentType::Html);
        assert_eq!(a, b);
    }

    #[test]
    fn discovered_url_not_equal_different_depth() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();

        let a = DiscoveredUrl::new(url.clone(), 0, parent.clone(), ContentType::Html);
        let b = DiscoveredUrl::new(url, 1, parent, ContentType::Html);
        assert_ne!(a, b);
    }

    #[test]
    fn discovered_url_not_equal_different_content_type() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();

        let a = DiscoveredUrl::new(url.clone(), 0, parent.clone(), ContentType::Html);
        let b = DiscoveredUrl::new(url, 0, parent, ContentType::Xml);
        assert_ne!(a, b);
    }

    #[test]
    fn discovered_url_all_content_types() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();

        for ct in [
            ContentType::Html,
            ContentType::Xml,
            ContentType::Text,
            ContentType::Other,
        ] {
            let discovered = DiscoveredUrl::new(url.clone(), 0, parent.clone(), ct);
            assert_eq!(discovered.content_type, ct);
        }
    }

    #[test]
    fn discovered_url_max_depth() {
        let url = Url::parse("https://example.com/deep").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::new(url, u8::MAX, parent, ContentType::Html);
        assert_eq!(discovered.depth, u8::MAX);
    }

    #[test]
    fn discovered_url_zero_depth_is_seed() {
        let url = Url::parse("https://example.com/").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::html(url, 0, parent);
        assert_eq!(discovered.depth, 0);
    }

    #[test]
    fn discovered_url_debug_output() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::html(url, 2, parent);
        let dbg = format!("{discovered:?}");
        assert!(dbg.contains("DiscoveredUrl"));
        assert!(dbg.contains("example.com"));
        assert!(dbg.contains("Html"));
    }

    #[test]
    fn discovered_url_clone() {
        let url = Url::parse("https://example.com/page").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();
        let discovered = DiscoveredUrl::new(url, 3, parent, ContentType::Text);
        let cloned = discovered.clone();
        assert_eq!(discovered, cloned);
    }

    #[test]
    fn discovered_url_not_equal_different_url() {
        let url_a = Url::parse("https://example.com/page-a").unwrap();
        let url_b = Url::parse("https://example.com/page-b").unwrap();
        let parent = Url::parse("https://example.com/").unwrap();

        let a = DiscoveredUrl::html(url_a, 1, parent.clone());
        let b = DiscoveredUrl::html(url_b, 1, parent);
        assert_ne!(a, b);
    }

    #[test]
    fn discovered_url_new_preserves_all_fields() {
        let url = Url::parse("https://example.com/article").unwrap();
        let parent = Url::parse("https://example.com/blog").unwrap();
        let discovered = DiscoveredUrl::new(url.clone(), 5, parent.clone(), ContentType::Text);

        assert_eq!(discovered.url, url);
        assert_eq!(discovered.depth, 5);
        assert_eq!(discovered.parent_url, parent);
        assert_eq!(discovered.content_type, ContentType::Text);
    }
}
