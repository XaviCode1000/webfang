//! Binary file utilities
//!
//! Functions for handling binary file downloads:
//! - Percent-decoding for filenames
//! - Deriving filenames from Content-Disposition headers
//! - Content-Disposition header parsing
//!
//! Extracted from discovery.rs to keep it orchestration-only.

use url::Url;

/// Maximum length for a derived filename component (ext4 per-file limit).
const MAX_FILENAME_LEN: usize = 255;

/// Simple percent-decoding for filenames (handles common cases).
///
/// Decodes percent-encoded characters in filenames, e.g. `%20` → space.
///
/// # Arguments
///
/// * `input` - String to decode
///
/// # Returns
///
/// Decoded string
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::crawler::binary_utils::percent_decode;
///
/// assert_eq!(percent_decode("file%20name.pdf"), "file name.pdf");
/// assert_eq!(percent_decode("no-encoding"), "no-encoding");
/// ```
#[inline]
#[must_use]
pub fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                result.push(byte as char);
            } else {
                result.push('%');
                result.push_str(&hex);
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Derive a filename from Content-Disposition header or URL path.
///
/// Priority: Content-Disposition `filename` > URL path basename > fallback.
///
/// # Arguments
///
/// * `headers` - HTTP response headers
/// * `url` - URL of the resource
/// * `content_type` - Content-Type header value
///
/// # Returns
///
/// Derived filename
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::crawler::binary_utils::derive_filename_from_response;
/// use url::Url;
///
/// let headers = wreq::header::HeaderMap::new();
/// let url = Url::parse("https://example.com/docs/report.pdf").unwrap();
/// let result = derive_filename_from_response(&headers, &url, "application/pdf");
/// assert_eq!(result, "report.pdf");
/// ```
pub fn derive_filename_from_response(
    headers: &wreq::header::HeaderMap,
    url: &Url,
    content_type: &str,
) -> String {
    // Try Content-Disposition header first (server-controlled: sanitized).
    if let Some(disposition) = headers.get(wreq::header::CONTENT_DISPOSITION) {
        if let Some(name) = disposition
            .to_str()
            .ok()
            .and_then(parse_content_disposition)
            .and_then(sanitize_disposition_filename)
        {
            return name;
        }
    }

    // Derive from URL path (also server-controlled: sanitize the same way)
    let path = url.path();
    let basename = path.rsplit('/').next().unwrap_or("");
    if !basename.is_empty() && basename != "/" {
        // Clean up the basename — remove query params that may be appended
        let clean = basename.split('?').next().unwrap_or(basename);
        if let Some(safe) = sanitize_filename_component(clean) {
            return safe;
        }
    }

    // Fallback: generate filename from content type
    let ext = match content_type {
        ct if ct.contains("application/pdf") => "pdf",
        ct if ct.contains("application/zip") => "zip",
        ct if ct.contains("application/x-tar") => "tar",
        ct if ct.contains("image/png") => "png",
        ct if ct.contains("image/jpeg") => "jpg",
        ct if ct.contains("image/gif") => "gif",
        ct if ct.contains("image/webp") => "webp",
        ct if ct.contains("image/svg") => "svg",
        ct if ct.contains("audio/mpeg") => "mp3",
        ct if ct.contains("video/mp4") => "mp4",
        _ => "bin",
    };

    // Use URL host + path hash for uniqueness
    let host = url.host_str().unwrap_or("unknown");
    let path_hash = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        path.hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };
    format!("{}_{}.{ext}", host.replace('.', "_"), &path_hash[..8])
}

/// Sanitize a parsed Content-Disposition filename, logging when the value is
/// neutralized (hostile input) or dropped entirely (nothing safe remains).
fn sanitize_disposition_filename(name: String) -> Option<String> {
    match sanitize_filename_component(&name) {
        Some(safe) => {
            if safe != name {
                tracing::warn!(
                    sanitized = %safe,
                    original_len = name.len(),
                    "hostile Content-Disposition filename neutralized to prevent path traversal"
                );
            }
            Some(safe)
        },
        None => {
            tracing::warn!(
                original_len = name.len(),
                "Content-Disposition filename fully hostile; falling back to URL-derived name"
            );
            None
        },
    }
}

/// Sanitize an untrusted derived filename into a safe single path component.
///
/// The result is guaranteed to be joinable onto a trusted directory without
/// escaping it:
///
/// - strips directory components (Unix `/` and Windows `\` separators), so
///   `../escape.bin` becomes `escape.bin` and absolute paths lose their root;
/// - drops `.` / `..` segments entirely (they resolve outside the target);
/// - removes control characters (including NUL bytes smuggled via RFC 5987
///   percent-decoding), which Unix filesystems reject;
/// - caps the length at [`MAX_FILENAME_LEN`] characters.
///
/// Returns `None` when nothing safe remains — callers apply their own
/// fallback naming (URL-derived or hash-based).
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::crawler::binary_utils::sanitize_filename_component;
///
/// assert_eq!(
///     sanitize_filename_component("../../escape.bin"),
///     Some("escape.bin".to_string())
/// );
/// assert_eq!(sanitize_filename_component(".."), None);
/// ```
#[must_use]
pub fn sanitize_filename_component(name: &str) -> Option<String> {
    // Remove control characters first (NUL included): they cannot appear in
    // Unix filenames and must never influence segment decisions.
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();

    let candidate = cleaned
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .next_back()
        .map(str::to_string)?;

    // Enforce the filesystem length cap on char boundaries.
    let mut capped = candidate;
    while capped.len() > MAX_FILENAME_LEN {
        capped.pop();
    }

    (!capped.is_empty()).then_some(capped)
}

/// Parse Content-Disposition header value to extract filename.
///
/// Supports:
/// - `filename="report.pdf"`
/// - `filename=report.pdf`
/// - `filename*=UTF-8''encoded-name.pdf`
///
/// # Arguments
///
/// * `value` - Content-Disposition header value
///
/// # Returns
///
/// Parsed filename or None
///
/// # Examples
///
/// ```
/// use webfang_core::infrastructure::crawler::binary_utils::parse_content_disposition;
///
/// assert_eq!(
///     parse_content_disposition("attachment; filename=\"report.pdf\""),
///     Some("report.pdf".to_string())
/// );
/// assert_eq!(
///     parse_content_disposition("attachment; filename*=UTF-8''encoded.pdf"),
///     Some("encoded.pdf".to_string())
/// );
/// ```
pub fn parse_content_disposition(value: &str) -> Option<String> {
    // Try filename*= first (RFC 5987 encoding)
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename*=") {
            // Format: UTF-8''encoded_name
            if let Some(name) = rest.strip_prefix("UTF-8''") {
                // Simple percent-decoding for common cases
                let decoded = percent_decode(name);
                if !decoded.is_empty() {
                    return Some(decoded);
                }
            }
        }
    }

    // Try filename= (standard)
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename=") {
            let name = rest.trim_matches(|c| c == '"' || c == '\'');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_decode_basic() {
        assert_eq!(percent_decode("file%20name.pdf"), "file name.pdf");
        assert_eq!(percent_decode("no-encoding"), "no-encoding");
        assert_eq!(percent_decode("%41%42%43"), "ABC");
    }

    #[test]
    fn test_percent_decode_invalid() {
        // Invalid hex should keep the original characters
        assert_eq!(percent_decode("test%ZZ"), "test%ZZ");
        assert_eq!(percent_decode("%"), "%");
        // Single hex digit after % is valid (e.g., %2 = char 2)
        assert_eq!(percent_decode("%2"), "\u{2}");
    }

    #[test]
    fn test_derive_filename_from_url_path() {
        let headers = wreq::header::HeaderMap::new();
        let url = Url::parse("https://example.com/docs/report.pdf").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "application/pdf");
        assert_eq!(result, "report.pdf");
    }

    #[test]
    fn test_derive_filename_from_content_disposition() {
        let mut headers = wreq::header::HeaderMap::new();
        headers.insert(
            wreq::header::CONTENT_DISPOSITION,
            "attachment; filename=\"invoice.pdf\""
                .parse()
                .expect("valid header value"),
        );
        let url = Url::parse("https://example.com/download").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "application/octet-stream");
        assert_eq!(result, "invoice.pdf");
    }

    #[test]
    fn test_derive_filename_fallback_pdf() {
        let headers = wreq::header::HeaderMap::new();
        let url = Url::parse("https://example.com/").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "application/pdf");
        assert!(
            result.ends_with(".pdf"),
            "Expected .pdf extension, got: {result}"
        );
    }

    #[test]
    fn test_derive_filename_fallback_png() {
        let headers = wreq::header::HeaderMap::new();
        let url = Url::parse("https://example.com/").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "image/png");
        assert!(
            result.ends_with(".png"),
            "Expected .png extension, got: {result}"
        );
    }

    #[test]
    fn test_derive_filename_fallback_unknown() {
        let headers = wreq::header::HeaderMap::new();
        let url = Url::parse("https://example.com/").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "text/plain");
        assert!(
            result.ends_with(".bin"),
            "Expected .bin extension, got: {result}"
        );
    }

    #[test]
    fn test_parse_content_disposition_filename() {
        let result = parse_content_disposition("attachment; filename=\"report.pdf\"");
        assert_eq!(result, Some("report.pdf".to_string()));
    }

    #[test]
    fn test_parse_content_disposition_filename_unquoted() {
        let result = parse_content_disposition("attachment; filename=report.pdf");
        assert_eq!(result, Some("report.pdf".to_string()));
    }

    #[test]
    fn test_parse_content_disposition_utf8() {
        let result = parse_content_disposition("attachment; filename*=UTF-8''encoded.pdf");
        assert_eq!(result, Some("encoded.pdf".to_string()));
    }

    #[test]
    fn test_parse_content_disposition_empty() {
        let result = parse_content_disposition("");
        assert_eq!(result, None);
    }

    #[test]
    fn test_parse_content_disposition_no_filename() {
        let result = parse_content_disposition("attachment");
        assert_eq!(result, None);
    }

    // -----------------------------------------------------------------
    // sanitize_filename_component — traversal hardening (batch 1)
    // -----------------------------------------------------------------

    #[test]
    fn sanitize_strips_traversal_segments() {
        assert_eq!(
            sanitize_filename_component("../escape.bin"),
            Some("escape.bin".to_string())
        );
        assert_eq!(sanitize_filename_component("a/../b"), Some("b".to_string()));
        assert_eq!(sanitize_filename_component(".."), None);
        assert_eq!(sanitize_filename_component("."), None);
    }

    #[test]
    fn sanitize_strips_directories_and_separators() {
        assert_eq!(
            sanitize_filename_component("/abs/path.bin"),
            Some("path.bin".to_string())
        );
        assert_eq!(
            sanitize_filename_component(r"\\server\share"),
            Some("share".to_string())
        );
    }

    #[test]
    fn sanitize_removes_null_bytes() {
        assert_eq!(
            sanitize_filename_component("nul\0byte"),
            Some("nulbyte".to_string())
        );
    }

    #[test]
    fn sanitize_caps_length_at_filesystem_limit() {
        let long = format!("{}.pdf", "a".repeat(300));
        let sanitized = sanitize_filename_component(&long).expect("non-empty result");
        assert!(sanitized.len() <= MAX_FILENAME_LEN);
        // Head-truncation: the hostile prefix is kept bounded, extension may
        // be cut — the join-safety guarantee is what matters.
        assert!(!sanitized.is_empty());
    }

    #[test]
    fn derive_neutralizes_hostile_content_disposition_name() {
        let mut headers = wreq::header::HeaderMap::new();
        headers.insert(
            wreq::header::CONTENT_DISPOSITION,
            "attachment; filename=\"../../escape.bin\""
                .parse()
                .expect("valid header value"),
        );
        let url = Url::parse("https://example.com/download").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "application/pdf");
        assert_eq!(result, "escape.bin");
    }

    #[test]
    fn derive_falls_back_to_url_basename_when_cd_fully_hostile() {
        let mut headers = wreq::header::HeaderMap::new();
        headers.insert(
            wreq::header::CONTENT_DISPOSITION,
            "attachment; filename=\"..\""
                .parse()
                .expect("valid header value"),
        );
        let url = Url::parse("https://example.com/docs/report.pdf").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "application/pdf");
        assert_eq!(result, "report.pdf");
    }

    #[test]
    fn derive_hash_fallback_is_safe_by_construction() {
        // Hostile CD + no URL basename → hash-based fallback: a single safe
        // component with no separators and no dot-segments.
        let mut headers = wreq::header::HeaderMap::new();
        headers.insert(
            wreq::header::CONTENT_DISPOSITION,
            "attachment; filename=\"..\""
                .parse()
                .expect("valid header value"),
        );
        let url = Url::parse("https://example.com/").expect("valid url");
        let result = derive_filename_from_response(&headers, &url, "application/pdf");
        assert!(result.ends_with(".pdf"));
        assert!(!result.contains('/'));
        assert!(!result.contains('\\'));
        assert_ne!(result, ".");
        assert_ne!(result, "..");
    }
}
