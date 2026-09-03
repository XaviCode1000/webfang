//! Binary file utilities — wreq adapter over the domain filename port (ADR-0012-B).
//!
//! The pure filename vocabulary (percent-decoding, Content-Disposition
//! parsing, sanitization, filename derivation) moved to
//! [`crate::domain::crawler_port::filename`] (ADR-0012-B unit 3). This module
//! keeps the historical re-export paths plus the thin
//! `wreq::header::HeaderMap` adapter — `wreq` is an HTTP-stack dependency and
//! must not enter the domain.

pub use crate::domain::crawler_port::filename::{
    derive_filename_from_content_disposition, parse_content_disposition, percent_decode,
    sanitize_filename_component,
};
use url::Url;

/// Derive a filename from a wreq response's headers or URL path.
///
/// Adapter over
/// [`derive_filename_from_content_disposition`]:
/// extracts the `Content-Disposition` header value (wreq canonicalizes header
/// names to lowercase) and delegates. Priority: header `filename` > URL path
/// basename > content-type fallback.
///
/// # Examples
///
/// ```
/// use url::Url;
/// use webfang_core::infrastructure::crawler::binary_utils::derive_filename_from_response;
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
    let content_disposition = headers
        .get(wreq::header::CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok());
    derive_filename_from_content_disposition(content_disposition, url, content_type)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::crawler_port::filename::MAX_FILENAME_LEN;

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

    /// #914 review finding #4: two distinct over-long names sharing a long
    /// common prefix must NOT collapse into the same truncated filename —
    /// that was a silent-overwrite vector.
    #[test]
    fn sanitize_disambiguates_distinct_names_with_long_common_prefix() {
        let prefix = "a".repeat(290);
        let first = format!("{prefix}tail1!.pdf");
        let second = format!("{prefix}tail2!.pdf");
        assert_eq!(first.len(), 300);
        assert_eq!(second.len(), 300);

        let sanitized_first = sanitize_filename_component(&first).expect("non-empty result");
        let sanitized_second = sanitize_filename_component(&second).expect("non-empty result");

        assert_ne!(
            sanitized_first, sanitized_second,
            "distinct inputs must produce distinct sanitized outputs"
        );
        assert!(sanitized_first.len() <= MAX_FILENAME_LEN);
        assert!(sanitized_second.len() <= MAX_FILENAME_LEN);
    }

    /// The disambiguation suffix must be deterministic: same input twice →
    /// identical output (stable filenames across crawl retries/resumes).
    #[test]
    fn sanitize_truncation_is_deterministic() {
        let long = format!("{}.pdf", "b".repeat(300));
        let once = sanitize_filename_component(&long).expect("non-empty result");
        let twice = sanitize_filename_component(&long).expect("non-empty result");
        assert_eq!(once, twice);
    }

    /// Short names (< cap) must be byte-identical to the pre-hash behavior.
    #[test]
    fn sanitize_short_names_unchanged_without_suffix() {
        assert_eq!(
            sanitize_filename_component("report.pdf"),
            Some("report.pdf".to_string())
        );
        assert_eq!(
            sanitize_filename_component("../../escape.bin"),
            Some("escape.bin".to_string())
        );
        assert_eq!(
            sanitize_filename_component("/abs/path.bin"),
            Some("path.bin".to_string())
        );
    }

    /// Multi-byte boundary safety: a name built from 4-byte emojis exceeding
    /// the cap must not panic mid-char and must respect the byte cap.
    #[test]
    fn sanitize_multibyte_truncation_respects_char_boundaries() {
        let emoji_name: String = "🦀".repeat(300); // 4 bytes each → 1200 bytes
        let sanitized = sanitize_filename_component(&emoji_name).expect("non-empty result");
        assert!(sanitized.len() <= MAX_FILENAME_LEN);
        // Whole chars only: no partial 4-byte sequence may survive.
        for (idx, _) in sanitized.char_indices() {
            assert!(sanitized.is_char_boundary(idx));
        }
        assert!(sanitized.is_char_boundary(sanitized.len()));
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
