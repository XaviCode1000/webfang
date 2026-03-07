//! MIME Type Detection Module
//!
//! Provides utilities for detecting file types from URLs and content.

#[cfg(any(feature = "images", feature = "documents"))]
use mimetype_detector::detect;

/// Detect MIME type from file extension
#[cfg(any(feature = "images", feature = "documents"))]
fn get_mime_from_extension(ext: &str) -> Option<&'static str> {
    let data = format!(".{}", ext);
    let mime = detect(data.as_bytes());
    Some(mime.mime())
}

/// Supported asset types for download
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetType {
    Image,
    Document,
    Unknown,
}

impl AssetType {
    /// Check if this is an image type
    pub fn is_image(&self) -> bool {
        matches!(self, AssetType::Image)
    }

    /// Check if this is a document type
    pub fn is_document(&self) -> bool {
        matches!(self, AssetType::Document)
    }
}

/// Known MIME types for images
#[allow(dead_code)]
const IMAGE_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "image/svg+xml",
    "image/bmp",
    "image/tiff",
    "image/x-icon",
];

/// Known MIME types for documents
#[allow(dead_code)]
const DOCUMENT_MIMES: &[&str] = &[
    "application/pdf",
    "application/msword",
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    "application/vnd.ms-excel",
    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    "application/vnd.ms-powerpoint",
    "application/vnd.openxmlformats-officedocument.presentationml.presentation",
    "text/csv",
    "application/vnd.oasis.opendocument.text",
    "application/vnd.oasis.opendocument.spreadsheet",
    "application/epub+zip",
    "application/rtf",
    "application/json",
    "application/xml",
    "text/xml",
];

/// Known file extensions for images
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "ico", "tiff", "tif",
];

/// Known file extensions for documents
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "odt", "ods", "odp", "epub", "rtf",
    "json", "xml",
];

#[cfg(any(feature = "images", feature = "documents"))]
/// Detect asset type from URL by extension
pub fn detect_from_url(url: &str) -> AssetType {
    // Parse URL and get path
    if let Ok(parsed) = url::Url::parse(url) {
        let path = parsed.path();
        return detect_from_path(path);
    }

    // Fallback: try to detect from the URL string itself
    AssetType::Unknown
}

#[cfg(not(any(feature = "images", feature = "documents")))]
/// Detect asset type from URL by extension (fallback without mimetype-detector)
pub fn detect_from_url(url: &str) -> AssetType {
    if let Ok(parsed) = url::Url::parse(url) {
        let path = parsed.path();
        return detect_from_path(path);
    }
    AssetType::Unknown
}

/// Detect asset type from file path
pub fn detect_from_path(path: &str) -> AssetType {
    // Get extension
    let extension = path
        .rsplit('.')
        .next()
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        AssetType::Image
    } else if DOCUMENT_EXTENSIONS.contains(&extension.as_str()) {
        AssetType::Document
    } else {
        AssetType::Unknown
    }
}

#[cfg(any(feature = "images", feature = "documents"))]
/// Detect asset type from raw bytes (magic bytes)
pub fn detect_from_bytes(data: &[u8]) -> AssetType {
    if data.is_empty() {
        return AssetType::Unknown;
    }

    let mime = detect(data);

    if IMAGE_MIMES.contains(&mime.mime()) {
        AssetType::Image
    } else if DOCUMENT_MIMES.contains(&mime.mime()) {
        AssetType::Document
    } else {
        AssetType::Unknown
    }
}

#[cfg(not(any(feature = "images", feature = "documents")))]
/// Detect asset type from raw bytes (fallback)
pub fn detect_from_bytes(_data: &[u8]) -> AssetType {
    // Without mimetype-detector, we can't detect from bytes
    AssetType::Unknown
}

/// Check if URL points to an image
pub fn is_image_url(url: &str) -> bool {
    detect_from_url(url).is_image()
}

/// Check if URL points to a document
pub fn is_document_url(url: &str) -> bool {
    detect_from_url(url).is_document()
}

/// Check if URL is a downloadable asset (image or document)
pub fn is_asset_url(url: &str) -> bool {
    let asset_type = detect_from_url(url);
    asset_type.is_image() || asset_type.is_document()
}

/// Get file extension from URL
pub fn get_extension(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.path().rsplit('.').next().map(|e| e.to_lowercase()))
}

/// Get MIME type from URL (basic detection by extension)
#[cfg(not(any(feature = "images", feature = "documents")))]
pub fn get_mime_type(url: &str) -> Option<&'static str> {
    let ext = get_extension(url)?;
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "pdf" => Some("application/pdf"),
        "doc" => Some("application/msword"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "xls" => Some("application/vnd.ms-excel"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "csv" => Some("text/csv"),
        "json" => Some("application/json"),
        "xml" => Some("application/xml"),
        _ => None,
    }
}

#[cfg(any(feature = "images", feature = "documents"))]
/// Get MIME type from URL using mimetype-detector
pub fn get_mime_type(url: &str) -> Option<&'static str> {
    // Try by extension first
    let ext = get_extension(url)?;
    get_mime_from_extension(&ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_image_from_url() {
        assert!(is_image_url("https://example.com/image.png"));
        assert!(is_image_url("https://example.com/photo.jpg"));
        assert!(is_image_url("https://example.com/diagram.webp"));
    }

    #[test]
    fn test_detect_document_from_url() {
        assert!(is_document_url("https://example.com/document.pdf"));
        assert!(is_document_url("https://example.com/report.docx"));
        assert!(is_document_url("https://example.com/data.xlsx"));
    }

    #[test]
    fn test_is_asset_url() {
        assert!(is_asset_url("https://example.com/image.png"));
        assert!(is_asset_url("https://example.com/doc.pdf"));
        assert!(!is_asset_url("https://example.com/page.html"));
    }

    #[test]
    fn test_get_extension() {
        assert_eq!(
            get_extension("https://example.com/file.png"),
            Some("png".to_string())
        );
        assert_eq!(
            get_extension("https://example.com/archive.tar.gz"),
            Some("gz".to_string())
        );
    }

    #[test]
    fn test_get_mime_type() {
        assert_eq!(
            get_mime_type("https://example.com/file.png"),
            Some("image/png")
        );
        assert_eq!(
            get_mime_type("https://example.com/file.pdf"),
            Some("application/pdf")
        );
    }
}
