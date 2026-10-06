//! MIME Type Detection Module
//!
//! Provides utilities for detecting file types from URLs and content.

/// Single canonical extension → MIME table.
///
/// #1813 T4: this function used to sit behind
/// `#[cfg(any(feature = "images", feature = "documents"))]`, paired with a
/// SECOND `get_mime_type` behind the negation of that same `any(...)`. Both
/// features were empty markers with no `dep:` entry, all three gates were
/// disjunctions, and `cfg(all(images, documents))` existed nowhere — so no
/// compiled configuration could tell the two features apart. The negated copy
/// answered 14 extensions; this one answers 26. Turning the feature off was
/// therefore not a configuration choice but a silent regression (12 extensions,
/// `txt` → `text/plain` among them, stopped resolving).
///
/// A Cargo feature earns its place by selecting a capability, justified by what
/// turning it off buys: binary size (both tables are static string maps, ~0.5 KB
/// total), compile time (no dependency is skipped), or behavior (turning it off
/// was strictly worse). None of the three applied, so both markers were
/// decoration that cost every reader a question with no answer. They are gone;
/// if a real capability-backed need appears — gating a heavy dependency, say —
/// add a feature then, with that dependency behind it.
fn get_mime_from_extension(ext: &str) -> Option<&'static str> {
    // Static mapping: this is a pure extension lookup. (The old doc comment
    // credited `mimetype-detector`, a crate this workspace has never depended
    // on; the map below is the only source of truth.)
    match ext.to_lowercase().as_str() {
        "jpg" | "jpeg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        "ico" => Some("image/x-icon"),
        "bmp" => Some("image/bmp"),
        "tiff" | "tif" => Some("image/tiff"),
        "pdf" => Some("application/pdf"),
        "doc" => Some("application/msword"),
        "docx" => Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
        "xls" => Some("application/vnd.ms-excel"),
        "xlsx" => Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"),
        "ppt" => Some("application/vnd.ms-powerpoint"),
        "pptx" => Some("application/vnd.openxmlformats-officedocument.presentationml.presentation"),
        "csv" => Some("text/csv"),
        "odt" => Some("application/vnd.oasis.opendocument.text"),
        "ods" => Some("application/vnd.oasis.opendocument.spreadsheet"),
        "odp" => Some("application/vnd.oasis.opendocument.presentation"),
        "epub" => Some("application/epub+zip"),
        "rtf" => Some("application/rtf"),
        "txt" => Some("text/plain"),
        "json" => Some("application/json"),
        "xml" => Some("application/xml"),
        _ => None,
    }
}

/// Supported asset types for download
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetType {
    /// Image file (JPEG, PNG, GIF, WebP, SVG, etc.)
    Image,
    /// Document file (PDF, DOC, XLS, CSV, etc.)
    Document,
    /// Unrecognized or non-asset file type
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

/// Known file extensions for images
const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "ico", "tiff", "tif",
];

/// Known file extensions for documents
const DOCUMENT_EXTENSIONS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "csv", "odt", "ods", "odp", "epub", "rtf",
    "json", "xml",
];

/// Detect asset type from URL by extension.
///
/// The two prior cfg-gated variants (with/without the `images`/`documents`
/// features) were byte-identical, so they are collapsed into one. The result
/// depends only on the URL path extension, never on `mimetype-detector`.
pub fn detect_from_url(url: &str) -> AssetType {
    // Parse URL and get path
    if let Ok(parsed) = url::Url::parse(url) {
        let path = parsed.path();
        return detect_from_path(path);
    }

    // Fallback: try to detect from the URL string itself
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

/// Get MIME type from URL (basic detection by extension).
///
/// #1813 T4: this was the third and last `cfg` site on the two dead markers,
/// and the only one whose documentation claimed a `mimetype-detector` lookup
/// this body never performed. Ungated for the reasons recorded on the private
/// `get_mime_from_extension` above; this is now the only definition.
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

    /// #1813 T4 — every extension the single table must answer.
    ///
    /// `test_get_mime_type` above asserts `png` and `pdf`, which are the two
    /// entries BOTH divergent copies shared. That overlap is precisely why the
    /// divergence went unnoticed, so the full list is pinned here instead:
    /// the 14 common entries plus the 12 the `not(any(...))` copy silently lost
    /// (`txt`, `ico`, `bmp`, `tiff`, `tif`, `ppt`, `pptx`, `odt`, `ods`,
    /// `odp`, `epub`, `rtf`).
    #[test]
    fn get_mime_type_answers_every_supported_extension() {
        let cases: &[(&str, &str)] = &[
            // Shared by the old common subset.
            ("file.jpg", "image/jpeg"),
            ("file.jpeg", "image/jpeg"),
            ("file.png", "image/png"),
            ("file.gif", "image/gif"),
            ("file.webp", "image/webp"),
            ("file.svg", "image/svg+xml"),
            ("file.pdf", "application/pdf"),
            ("file.doc", "application/msword"),
            (
                "file.docx",
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            ("file.xls", "application/vnd.ms-excel"),
            (
                "file.xlsx",
                "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            ),
            ("file.csv", "text/csv"),
            ("file.json", "application/json"),
            ("file.xml", "application/xml"),
            // Lost by the removed feature-gated copy — the actual regression.
            ("file.ico", "image/x-icon"),
            ("file.bmp", "image/bmp"),
            ("file.tiff", "image/tiff"),
            ("file.tif", "image/tiff"),
            ("file.ppt", "application/vnd.ms-powerpoint"),
            (
                "file.pptx",
                "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            ),
            ("file.odt", "application/vnd.oasis.opendocument.text"),
            ("file.ods", "application/vnd.oasis.opendocument.spreadsheet"),
            (
                "file.odp",
                "application/vnd.oasis.opendocument.presentation",
            ),
            ("file.epub", "application/epub+zip"),
            ("file.rtf", "application/rtf"),
            ("file.txt", "text/plain"),
        ];
        for (name, expected) in cases {
            assert_eq!(
                get_mime_type(&format!("https://example.com/{name}")),
                Some(*expected),
                "MIME type for {name}"
            );
        }
    }

    /// Triangulation: the table is answered case-insensitively, and anything
    /// outside it — an unknown extension or a path with no extension at all —
    /// is `None` rather than a guess.
    #[test]
    fn get_mime_type_is_case_insensitive_and_rejects_the_unknown() {
        assert_eq!(
            get_mime_type("https://example.com/FILE.TXT"),
            Some("text/plain")
        );
        assert_eq!(get_mime_type("https://example.com/file.exe"), None);
        assert_eq!(get_mime_type("https://example.com/download"), None);
    }

    // ============================================================================
    // Error path tests
    // ============================================================================

    #[test]
    fn test_detect_unknown_no_extension() {
        // URL like `https://example.com/download` has no extension → Unknown
        let asset_type = detect_from_url("https://example.com/download");
        assert_eq!(asset_type, AssetType::Unknown);
        assert!(!asset_type.is_image());
        assert!(!asset_type.is_document());
    }

    #[test]
    fn test_detect_case_insensitive() {
        // .PNG should be detected same as .png
        let png_upper = detect_from_url("https://example.com/image.PNG");
        let png_lower = detect_from_url("https://example.com/image.png");
        assert_eq!(png_upper, png_lower);
        assert!(png_upper.is_image());

        // .PDF should be detected same as .pdf
        let pdf_upper = detect_from_url("https://example.com/doc.PDF");
        let pdf_lower = detect_from_url("https://example.com/doc.pdf");
        assert_eq!(pdf_upper, pdf_lower);
        assert!(pdf_upper.is_document());
    }

    #[test]
    fn test_detect_invalid_url() {
        // Invalid URL should return Unknown
        let asset_type = detect_from_url("not-a-valid-url-at-all");
        assert_eq!(asset_type, AssetType::Unknown);
    }
}
