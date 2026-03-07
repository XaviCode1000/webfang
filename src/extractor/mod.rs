//! Asset Extraction Module
//!
//! Extracts URLs of images and documents from HTML content.

use scraper::{Html, Selector};

/// Represents an extracted asset URL
#[derive(Debug, Clone)]
pub struct AssetUrl {
    /// The full URL of the asset
    pub url: String,
    /// Asset type (image or document)
    pub asset_type: crate::detector::AssetType,
    /// Optional alt text (for images)
    pub alt: Option<String>,
}

/// Extract all image URLs from HTML
pub fn extract_images(html: &str, base_url: &url::Url) -> Vec<AssetUrl> {
    let document = Html::parse_document(html);
    let mut assets = Vec::new();

    // Extract from <img> tags
    let img_selector = Selector::parse("img[src]").unwrap();
    for img in document.select(&img_selector) {
        if let Some(src) = img.value().attr("src") {
            if !src.is_empty() && !src.starts_with("data:") {
                let absolute_url = resolve_url(base_url, src);
                if let Some(url) = absolute_url {
                    let alt = img.value().attr("alt").map(String::from);
                    let asset_type = crate::detector::detect_from_url(&url);
                    if asset_type.is_image() {
                        assets.push(AssetUrl {
                            url,
                            asset_type,
                            alt,
                        });
                    }
                }
            }
        }
    }

    // Extract from <img srcset="...">
    let srcset_selector = Selector::parse("img[srcset]").unwrap();
    for img in document.select(&srcset_selector) {
        if let Some(srcset) = img.value().attr("srcset") {
            for src in parse_srcset(srcset) {
                let absolute_url = resolve_url(base_url, &src);
                if let Some(url) = absolute_url {
                    let asset_type = crate::detector::detect_from_url(&url);
                    if asset_type.is_image() {
                        assets.push(AssetUrl {
                            url,
                            asset_type,
                            alt: None,
                        });
                    }
                }
            }
        }
    }

    // Extract from <picture><source srcset="...">
    let source_selector = Selector::parse("source[srcset]").unwrap();
    for source in document.select(&source_selector) {
        if let Some(srcset) = source.value().attr("srcset") {
            for src in parse_srcset(srcset) {
                let absolute_url = resolve_url(base_url, &src);
                if let Some(url) = absolute_url {
                    let asset_type = crate::detector::detect_from_url(&url);
                    if asset_type.is_image() {
                        assets.push(AssetUrl {
                            url,
                            asset_type,
                            alt: None,
                        });
                    }
                }
            }
        }
    }

    // Extract from <figure> with <img>
    let figure_selector = Selector::parse("figure img[src]").unwrap();
    for img in document.select(&figure_selector) {
        if let Some(src) = img.value().attr("src") {
            if !src.is_empty() && !src.starts_with("data:") {
                let absolute_url = resolve_url(base_url, src);
                if let Some(url) = absolute_url {
                    let alt = img.value().attr("alt").map(String::from);
                    let asset_type = crate::detector::detect_from_url(&url);
                    if asset_type.is_image() {
                        assets.push(AssetUrl {
                            url,
                            asset_type,
                            alt,
                        });
                    }
                }
            }
        }
    }

    assets
}

/// Extract all document URLs from HTML
pub fn extract_documents(html: &str, base_url: &url::Url) -> Vec<AssetUrl> {
    let document = Html::parse_document(html);
    let mut assets = Vec::new();

    // Extensions to look for in links
    #[allow(unused)]
    let doc_extensions = [
        "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "odt", "ods", "odp", "epub",
        "rtf",
    ];

    // Extract from <a> tags
    let link_selector = Selector::parse("a[href]").unwrap();
    for link in document.select(&link_selector) {
        if let Some(href) = link.value().attr("href") {
            if !href.is_empty() && !href.starts_with('#') && !href.starts_with("javascript:") {
                let absolute_url = resolve_url(base_url, href);
                if let Some(url) = absolute_url {
                    let asset_type = crate::detector::detect_from_url(&url);
                    if asset_type.is_document() {
                        // Get link text as description
                        let text = link.text().collect::<String>().trim().to_string();
                        let description = if text.is_empty() { None } else { Some(text) };

                        assets.push(AssetUrl {
                            url,
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
pub fn extract_all_assets(html: &str, base_url: &url::Url) -> Vec<AssetUrl> {
    let mut assets = Vec::new();
    assets.extend(extract_images(html, base_url));
    assets.extend(extract_documents(html, base_url));
    assets
}

/// Resolve a relative URL against a base URL
fn resolve_url(base_url: &url::Url, relative_url: &str) -> Option<String> {
    // Handle protocol-relative URLs (//example.com/file.pdf)
    if relative_url.starts_with("//") {
        let resolved = format!("{}:{}", base_url.scheme(), relative_url);
        return Some(resolved);
    }

    // Handle absolute URLs
    if relative_url.starts_with("http://") || relative_url.starts_with("https://") {
        return Some(relative_url.to_string());
    }

    // Handle root-relative URLs
    if relative_url.starts_with('/') {
        if let Ok(base) = url::Url::parse(&format!(
            "{}://{}",
            base_url.scheme(),
            base_url.host_str().unwrap_or("")
        )) {
            return base.join(relative_url).ok().map(|u| u.to_string());
        }
    }

    // Handle relative URLs
    base_url.join(relative_url).ok().map(|u| u.to_string())
}

/// Parse srcset attribute
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

#[cfg(test)]
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
    fn test_resolve_url() {
        let base = url::Url::parse("https://example.com/path/page.html").unwrap();

        assert_eq!(
            resolve_url(&base, "image.png"),
            Some("https://example.com/path/image.png".to_string())
        );

        assert_eq!(
            resolve_url(&base, "/assets/style.css"),
            Some("https://example.com/assets/style.css".to_string())
        );

        assert_eq!(
            resolve_url(&base, "https://other.com/file.pdf"),
            Some("https://other.com/file.pdf".to_string())
        );
    }

    #[test]
    fn test_extract_images() {
        let html = r#"<html><body>
            <img src="/images/photo.jpg" alt="A photo">
            <img src="https://cdn.example.com/logo.png">
        </body></html>"#;

        let base = url::Url::parse("https://example.com/").unwrap();
        let images = extract_images(html, &base);

        assert_eq!(images.len(), 2);
    }

    #[test]
    fn test_extract_documents() {
        let html = r#"<html><body>
            <a href="/docs/report.pdf">Annual Report</a>
            <a href="https://company.com/data.xlsx">Data</a>
        </body></html>"#;

        let base = url::Url::parse("https://example.com/").unwrap();
        let docs = extract_documents(html, &base);

        assert_eq!(docs.len(), 2);
    }
}
