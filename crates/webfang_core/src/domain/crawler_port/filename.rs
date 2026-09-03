//! Filename derivation port — pure filename vocabulary in domain (ADR-0012-B).
//!
//! Extracted from `infrastructure::crawler::binary_utils` so application code
//! can derive download filenames from plain `HashMap<String, String>` headers
//! (the `FetchedPage.headers` contract: lowercased keys) without an
//! `application→infrastructure` edge (ADR-0010). Infrastructure keeps
//! `binary_utils` as a `pub use` shim plus the wreq `HeaderMap` adapter —
//! `wreq` must not enter the domain.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use url::Url;

/// Maximum length for a derived filename component (ext4 per-file limit).
pub(crate) const MAX_FILENAME_LEN: usize = 255;

/// Simple percent-decoding for filenames (`%20` → space; invalid hex kept).
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

/// Parse Content-Disposition header value to extract filename.
///
/// Supports `filename="report.pdf"`, `filename=report.pdf`, and the RFC 5987
/// form `filename*=UTF-8''encoded-name.pdf`.
pub fn parse_content_disposition(value: &str) -> Option<String> {
    // Try filename*= first (RFC 5987 encoding)
    for part in value.split(';') {
        let part = part.trim();
        if let Some(rest) = part.strip_prefix("filename*=") {
            if let Some(name) = rest.strip_prefix("UTF-8''") {
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

/// Sanitize an untrusted derived filename into a safe single path component.
///
/// Strips `/` and `\` separators, drops `.` / `..` segments, removes control
/// characters, and caps the length at `MAX_FILENAME_LEN` (255) bytes. When
/// the cap fires, a deterministic `DefaultHasher`-based suffix keeps distinct
/// over-long names distinct (#914); inputs under the cap are byte-identical
/// to their input. Returns `None` when nothing safe remains (`.` / `..`).
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

    if candidate.len() <= MAX_FILENAME_LEN {
        return (!candidate.is_empty()).then_some(candidate);
    }

    // Capping fires: append a deterministic suffix derived from the ORIGINAL
    // candidate ("-" + 8 lowercase hex chars = 9 bytes) so distinct long names
    // sharing a common prefix stay distinct after truncation (#914).
    let mut hasher = DefaultHasher::new();
    candidate.hash(&mut hasher);
    let hash8 = format!("{:08x}", hasher.finish());

    // Reserve room for "-<hash8>" and truncate the prefix on a CHAR boundary.
    let max_prefix_bytes = MAX_FILENAME_LEN - 1 - hash8.len();
    let mut truncated = String::with_capacity(MAX_FILENAME_LEN);
    let mut used = 0usize;
    for c in candidate.chars() {
        let char_len = c.len_utf8();
        if used + char_len > max_prefix_bytes {
            break;
        }
        truncated.push(c);
        used += char_len;
    }
    truncated.push('-');
    truncated.push_str(&hash8);

    debug_assert!(truncated.len() <= MAX_FILENAME_LEN);

    (!truncated.is_empty()).then_some(truncated)
}

/// Contain an untrusted name inside its parent directory (#1125).
///
/// Neutralizes separators (`/` and `\`), `.` / `..` segments and control
/// characters through [`sanitize_filename_component`], returning `fallback`
/// when nothing safe remains. Emits a `warn!` event whenever the input had
/// to change, so hostile inputs stay observable. `fallback` must be a safe
/// literal (no separators); it is returned verbatim.
///
/// Idempotent: confining an already-safe name returns it byte-identical.
#[must_use]
pub fn confine_filename_component(raw: &str, fallback: &str) -> String {
    match sanitize_filename_component(raw) {
        Some(safe) => {
            if safe != raw {
                tracing::warn!(
                    original_len = raw.len(),
                    sanitized = %safe,
                    "hostile filename neutralized to prevent path traversal"
                );
            }
            safe
        },
        None => {
            tracing::warn!(
                original_len = raw.len(),
                fallback,
                "filename fully hostile; using fallback to prevent path traversal"
            );
            fallback.to_string()
        },
    }
}

/// Derive a filename from the Content-Disposition header value or URL path.
///
/// Priority: Content-Disposition `filename` > URL path basename >
/// content-type fallback (`<host>_<path-hash>.<ext>`). `content_disposition`
/// is the raw header value or `None` when absent — callers pass
/// `page.headers.get("content-disposition")` directly.
pub fn derive_filename_from_content_disposition(
    content_disposition: Option<&str>,
    url: &Url,
    content_type: &str,
) -> String {
    // Try Content-Disposition header first (server-controlled: sanitized).
    if let Some(disposition) = content_disposition {
        if let Some(name) =
            parse_content_disposition(disposition).and_then(sanitize_disposition_filename)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confine_keeps_safe_names_byte_identical() {
        assert_eq!(confine_filename_component("export", "export"), "export");
        assert_eq!(
            confine_filename_component("example.com", "unknown"),
            "example.com"
        );
    }

    #[test]
    fn confine_neutralizes_traversal_vectors() {
        // Issue #1125 proof vectors: every hostile input collapses to a
        // single safe component that cannot escape its parent directory.
        assert_eq!(confine_filename_component("../escape", "export"), "escape");
        assert_eq!(confine_filename_component("sub/out", "export"), "out");
        assert_eq!(confine_filename_component("..\\escape", "export"), "escape");
        assert_eq!(confine_filename_component("..", "export"), "export");
        assert_eq!(confine_filename_component("a/../b", "export"), "b");
    }
}
