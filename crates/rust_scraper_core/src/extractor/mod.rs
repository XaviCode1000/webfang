//! Asset Extraction Module
//!
//! Extracts URLs of images and documents from HTML content.

use scraper::{Html, Selector};
use std::sync::LazyLock;

// CSS selectors - compilados una sola vez con LazyLock (opt-inline, err-no-unwrap-prod)
/// Selector for img[src]
static IMG_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("img[src]").expect("BUG: invalid CSS selector img[src]"));

/// Selector for img[srcset]
static SRCSET_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("img[srcset]").expect("BUG: invalid CSS selector img[srcset]")
});

/// Selector for source[srcset]
static SOURCE_SELECTOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("source[srcset]").expect("BUG: invalid CSS selector source[srcset]")
});

/// Selector for a[href]
static LINK_SELECTOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a[href]").expect("BUG: invalid CSS selector a[href]"));

/// Represents an extracted asset URL
#[derive(Debug, Clone)]
pub struct AssetUrl {
    /// The full URL of the asset
    pub url: String,
    /// Asset type (image or document)
    pub asset_type: crate::adapters::detector::AssetType,
    /// Optional alt text (for images)
    pub alt: Option<String>,
}

/// Extract all image URLs from HTML document
///
/// # Arguments
///
/// * `document` - Parsed HTML document reference (no clone)
/// * `base_url` - Base URL for resolving relative URLs
///
/// # Returns
///
/// Vector of asset URLs with type and alt text
#[must_use]
pub fn extract_images(document: &Html, base_url: &url::Url) -> Vec<AssetUrl> {
    let mut assets = Vec::new();

    // Extract from <img> tags
    for img in document.select(&IMG_SELECTOR) {
        if let Some(src) = img.value().attr("src") {
            if let Some(asset) =
                process_asset_src(src, base_url, img.value().attr("alt").map(String::from))
            {
                if asset.asset_type.is_image() {
                    assets.push(asset);
                }
            }
        }
    }

    // Extract from <img srcset="...">
    for img in document.select(&SRCSET_SELECTOR) {
        if let Some(srcset) = img.value().attr("srcset") {
            for src in parse_srcset(srcset) {
                if let Some(asset) = process_asset_src(&src, base_url, None) {
                    if asset.asset_type.is_image() {
                        assets.push(asset);
                    }
                }
            }
        }
    }

    // Extract from <picture><source srcset="...">
    for source in document.select(&SOURCE_SELECTOR) {
        if let Some(srcset) = source.value().attr("srcset") {
            for src in parse_srcset(srcset) {
                if let Some(asset) = process_asset_src(&src, base_url, None) {
                    if asset.asset_type.is_image() {
                        assets.push(asset);
                    }
                }
            }
        }
    }

    assets
}

/// Extract all document URLs from HTML document
///
/// # Arguments
///
/// * `document` - Parsed HTML document reference (no clone)
/// * `base_url` - Base URL for resolving relative URLs
///
/// # Returns
///
/// Vector of document asset URLs with description
#[must_use]
pub fn extract_documents(document: &Html, base_url: &url::Url) -> Vec<AssetUrl> {
    let mut assets = Vec::new();

    // Extract from <a> tags
    for link in document.select(&LINK_SELECTOR) {
        if let Some(href) = link.value().attr("href") {
            if !href.is_empty() && !href.starts_with('#') && !href.starts_with("javascript:") {
                // Skip data: and javascript: URLs
                if href.starts_with("data:") {
                    continue;
                }

                if let Ok(absolute_url) = base_url.join(href) {
                    let absolute_url = absolute_url.to_string();
                    let asset_type = crate::adapters::detector::detect_from_url(&absolute_url);

                    if asset_type.is_document() {
                        // Get link text as description
                        let text = link.text().collect::<String>().trim().to_owned();
                        let description = if text.is_empty() { None } else { Some(text) };

                        assets.push(AssetUrl {
                            url: absolute_url,
                            asset_type,
                            alt: description,
                        });
                    }
                }
            }
        }
    }

    assets
}

/// Extract all asset URLs (images + documents) from HTML
///
/// Parses HTML only ONCE and reuses the document for both extractors.
///
/// # Arguments
///
/// * `html` - Raw HTML string
/// * `base_url` - Base URL for resolving relative URLs
///
/// # Returns
///
/// Vector of all asset URLs
#[must_use]
pub fn extract_all_assets(html: &str, base_url: &url::Url) -> Vec<AssetUrl> {
    let document = Html::parse_document(html);
    let mut assets = extract_images(&document, base_url);
    assets.extend(extract_documents(&document, base_url));
    assets
}

/// Process an asset source URL with validation
///
/// Validates and resolves asset URLs, filtering out invalid sources.
///
/// # Arguments
///
/// * `src` - Source URL (relative or absolute)
/// * `base_url` - Base URL for resolution
/// * `alt` - Optional alt text
///
/// # Returns
///
/// Some(AssetUrl) if valid, None if filtered out
#[inline]
fn process_asset_src(src: &str, base_url: &url::Url, alt: Option<String>) -> Option<AssetUrl> {
    // Filter out invalid sources
    if src.is_empty()
        || src.starts_with("data:")
        || src.starts_with("javascript:")
        || src.starts_with('#')
    {
        return None;
    }

    // Resolve URL using Url::join (RFC 3986 compliant)
    let url = base_url.join(src).ok()?.to_string();
    let asset_type = crate::adapters::detector::detect_from_url(&url);
    Some(AssetUrl {
        url,
        asset_type,
        alt,
    })
}

/// Parse srcset attribute
///
/// Format: "url1 1x, url2 2x" or "url1 100w, url2 200w"
fn parse_srcset(srcset: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for part in srcset.split(',') {
        let part = part.trim();
        if let Some(url) = part.split_whitespace().next() {
            if !url.is_empty() {
                urls.push(url.to_string());
            }
        }
    }
    urls
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn test_parse_srcset() {
        let srcset = "image-320.jpg 320w, image-640.jpg 640w, image-1280.jpg 1280w";
        let urls = parse_srcset(srcset);
        assert_eq!(urls.len(), 3);
        assert!(urls.contains(&"image-320.jpg".to_string()));
    }

    #[test]
    fn test_process_asset_src_valid() {
        let base = url::Url::parse("https://example.com/path/").unwrap();
        let asset = process_asset_src("image.png", &base, Some("test".to_string()));

        assert!(asset.is_some());
        let asset = asset.unwrap();
        assert_eq!(asset.url, "https://example.com/path/image.png");
        assert_eq!(asset.alt, Some("test".to_string()));
    }

    #[test]
    fn test_process_asset_src_invalid() {
        let base = url::Url::parse("https://example.com/").unwrap();

        // Empty
        assert!(process_asset_src("", &base, None).is_none());

        // Data URLs
        assert!(process_asset_src("data:image/png;base64,abc", &base, None).is_none());

        // JavaScript URLs
        assert!(process_asset_src("javascript:alert(1)", &base, None).is_none());

        // Fragment URLs
        assert!(process_asset_src("#section", &base, None).is_none());
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_images() {
        let html = r#"<html><body>
            <img src="/images/photo.jpg" alt="A photo">
            <img src="https://cdn.example.com/logo.png">
        </body></html>"#;

        let base = url::Url::parse("https://example.com/").unwrap();
        let document = Html::parse_document(html);
        let images = extract_images(&document, &base);

        assert_eq!(images.len(), 2);
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_documents() {
        let html = r#"<html><body>
            <a href="/docs/report.pdf">Annual Report</a>
            <a href="https://company.com/data.xlsx">Data</a>
        </body></html>"#;

        let base = url::Url::parse("https://example.com/").unwrap();
        let document = Html::parse_document(html);
        let docs = extract_documents(&document, &base);

        assert_eq!(docs.len(), 2);
    }

    #[cfg_attr(miri, ignore)] // scraper::Selector servo_arc UB
    #[test]
    fn test_extract_all_assets_single_parse() {
        let html = r#"<html><body>
            <img src="/images/photo.jpg" alt="A photo">
            <a href="/docs/report.pdf">Report</a>
        </body></html>"#;

        let base = url::Url::parse("https://example.com/").unwrap();
        let assets = extract_all_assets(html, &base);

        // Should have 1 image + 1 document
        assert_eq!(assets.len(), 2);
    }
}
