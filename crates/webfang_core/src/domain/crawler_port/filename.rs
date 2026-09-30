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

/// Windows reserved device names (case-insensitive).
/// <https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file>
///
/// These names cannot be used as file names on Windows, regardless of
/// extension. Attempting to create files with these names will crash on
/// Windows.
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Suffix appended when a reserved name must be neutralized. Matches the
/// convention of `UrlPath::to_safe_filename_with_format` (`CON` → `CON_safe`).
const RESERVED_SAFE_SUFFIX: &str = "_safe";

/// Characters that cannot survive a Windows path component.
///
/// The first six are outright illegal on Windows (`Win32` naming rules). `:` is
/// the dangerous one (XP-P-05, issue #1608): on NTFS it is not rejected but
/// reinterpreted as the ALTERNATE DATA STREAM separator, so
/// `Content-Disposition: filename="report.txt:hidden"` creates a **0-byte**
/// `report.txt` whose real content lives in a stream — the operation reports
/// success and the download silently vanishes.
///
/// These are SUBSTITUTED rather than rejected: the input here is server-
/// controlled (Content-Disposition / URL path) and the function must return a
/// usable name, so it neutralizes instead of failing.
const WINDOWS_INVALID_COMPONENT_CHARS: &[char] = &['<', '>', '"', '|', '?', '*', ':'];

/// Replacement for every character in [`WINDOWS_INVALID_COMPONENT_CHARS`].
///
/// `_` is already legal on every supported filesystem, so substituting it needs
/// no second rule. Distinct inputs can collide after substitution (`a:b` and
/// `a_b`); the downloader layer's existing collision resolution and the
/// hash-suffix path below own disambiguation, exactly as they do for long names.
const INVALID_CHAR_REPLACEMENT: char = '_';

/// True when the STEM of `name` (everything before the FIRST `.`) matches a
/// Windows reserved device name, ASCII case-insensitively.
///
/// Windows treats everything up to the first period as the device name, so
/// `CON.txt` is just as unusable as `CON`; `docs-page-CON` (no dot) is NOT
/// reserved because its whole string is the stem and does not match.
///
/// Shared vocabulary for every filename-derivation surface in the workspace:
/// [`sanitize_filename_component`] (this module) neutralizes reserved stems,
/// and `adapters::url_path` uses the same check for URL-derived filenames
/// (issue #1608).
#[must_use]
pub fn is_windows_reserved(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    let upper = stem.to_ascii_uppercase();
    WINDOWS_RESERVED.contains(&upper.as_str())
}

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
///
/// Windows reserved device names (XP-P-04, issue #1608): a candidate whose
/// stem is reserved (`CON`, `CON.txt`, ...) gets the `"_safe"` suffix
/// appended so the derived name is creatable on Windows hosts. The check
/// runs BEFORE the length cap, so a suffixed name over the cap still goes
/// through the hash-truncation path.
///
/// Windows-illegal characters (XP-P-05, issue #1608): every char in
/// [`WINDOWS_INVALID_COMPONENT_CHARS`] — including the NTFS
/// alternate-data-stream `:` — is substituted with [`INVALID_CHAR_REPLACEMENT`]
/// so a server-supplied name can never create a stream or an uncreatable file.
/// Previously only the MCP boundary rejected these; the crawler/export
/// download path (Content-Disposition and URL-derived names) passed them
/// straight through, which is where the 0-byte-main-stream outcome lived.
///
/// Windows trailing dot/space (XP-P-06, issue #1608): NTFS silently trims
/// both, so `report.` and `report` are the SAME file there while being two on
/// Linux. Trimming here makes both platforms agree instead of producing a
/// name that collides only after the export crosses a filesystem boundary.
///
/// Ordering is load-bearing: neutralize chars, THEN trim, THEN the reserved
/// stem check (`CON.` must become `CON_safe`, not `CON._safe`), THEN the cap.
#[must_use]
pub fn sanitize_filename_component(name: &str) -> Option<String> {
    // Remove control characters first (NUL included): they cannot appear in
    // Unix filenames and must never influence segment decisions.
    let cleaned: String = name.chars().filter(|c| !c.is_control()).collect();

    let candidate = cleaned
        .split(['/', '\\'])
        .rfind(|segment| !segment.is_empty() && *segment != "." && *segment != "..")
        .map(str::to_string)?;

    let candidate = neutralize_windows_invalid_chars(&candidate);
    let candidate = trim_windows_trailing_dots_and_spaces(&candidate);
    if candidate.is_empty() {
        return None;
    }

    let mut candidate = candidate.to_string();
    if is_windows_reserved(&candidate) {
        candidate.push_str(RESERVED_SAFE_SUFFIX);
    }

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

/// Substitute every Windows-illegal component character with
/// [`INVALID_CHAR_REPLACEMENT`] (XP-P-05).
fn neutralize_windows_invalid_chars(name: &str) -> String {
    if !name.contains(WINDOWS_INVALID_COMPONENT_CHARS) {
        return name.to_string();
    }
    name.chars()
        .map(|c| {
            if WINDOWS_INVALID_COMPONENT_CHARS.contains(&c) {
                INVALID_CHAR_REPLACEMENT
            } else {
                c
            }
        })
        .collect()
}

/// Trim the trailing dots and spaces NTFS discards on its own (XP-P-06).
///
/// Borrowed, not allocated: the callers only need the view, and a name made
/// entirely of trim-able characters yields an empty slice, which the caller
/// turns into `None` (nothing safe remains — `...` is not a creatable file on
/// any supported platform).
fn trim_windows_trailing_dots_and_spaces(name: &str) -> &str {
    name.trim_end_matches(['.', ' '])
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

    // --- Windows reserved stems (issue #1608, XP-P-04) ----------------------

    #[test]
    fn is_windows_reserved_matches_stem_case_insensitively() {
        assert!(is_windows_reserved("CON"));
        assert!(is_windows_reserved("con"));
        assert!(is_windows_reserved("Con"));
        assert!(is_windows_reserved("CON.txt"));
        assert!(is_windows_reserved("nul.tar.gz"));
        assert!(is_windows_reserved("com1"));
        assert!(is_windows_reserved("lpt9.md"));
        // No dot: the whole string is the stem.
        assert!(!is_windows_reserved("docs-page-CON"));
        // Stem before the FIRST dot does not match.
        assert!(!is_windows_reserved("document.2026"));
        assert!(!is_windows_reserved("config"));
        // lookalikes that are not reserved
        assert!(!is_windows_reserved("console"));
        assert!(!is_windows_reserved("com10"));
    }

    #[test]
    fn sanitize_neutralizes_reserved_stems() {
        assert_eq!(
            sanitize_filename_component("CON"),
            Some("CON_safe".to_string())
        );
        assert_eq!(
            sanitize_filename_component("con.txt"),
            Some("con.txt_safe".to_string())
        );
        // Case-insensitive, mixed case preserved.
        assert_eq!(
            sanitize_filename_component("Nul"),
            Some("Nul_safe".to_string())
        );
        // Non-reserved names stay byte-identical.
        assert_eq!(
            sanitize_filename_component("document.txt"),
            Some("document.txt".to_string())
        );
    }

    #[test]
    fn confine_reserved_name_is_idempotent() {
        // First pass neutralizes (and warns via safe != raw); the output is
        // itself safe, so a second pass must return it byte-identical.
        let once = confine_filename_component("CON", "export");
        assert_eq!(once, "CON_safe");
        assert_eq!(confine_filename_component(&once, "export"), once);
    }

    // --- NTFS hazards in the DOWNLOAD path (issue #1608, XP-P-05/XP-P-06) ---
    //
    // The MCP boundary (mcp_server/validation.rs) already rejected these; the
    // crawler/export download path did not, and it is the one fed by
    // server-controlled Content-Disposition and URL-path names.

    #[test]
    fn sanitize_substitutes_ntfs_alternate_data_stream_colon() {
        // XP-P-05: `:` must never survive into a component — on NTFS it opens
        // a stream and leaves the main file 0 bytes.
        assert_eq!(
            sanitize_filename_component("report.txt:hidden"),
            Some("report.txt_hidden".to_string())
        );
        assert_eq!(
            sanitize_filename_component("a:b:c"),
            Some("a_b_c".to_string())
        );
        // A bare ADS-style name that reduces to nothing usable returns None so
        // the caller falls through to the next derivation source.
        assert_eq!(sanitize_filename_component(":"), Some("_".to_string()));
    }

    #[test]
    fn sanitize_substitutes_windows_invalid_charset() {
        // Illegal on Windows, ordinary bytes on Linux — the exact asymmetry
        // that made derived exports non-portable.
        assert_eq!(
            sanitize_filename_component("a<b>c|d?e*f"),
            Some("a_b_c_d_e_f".to_string())
        );
        assert_eq!(
            sanitize_filename_component("informe \"final\".pdf"),
            Some("informe _final_.pdf".to_string())
        );
    }

    #[test]
    fn sanitize_trims_windows_trailing_dots_and_spaces() {
        // XP-P-06: NTFS trims these silently, so accepting them creates a name
        // that collides with the trimmed form only after the filesystem is
        // crossed. Trimming makes both platforms agree.
        assert_eq!(
            sanitize_filename_component("report."),
            Some("report".to_string())
        );
        assert_eq!(
            sanitize_filename_component("report  "),
            Some("report".to_string())
        );
        // Order is load-bearing: a reserved stem is checked AFTER trimming, so
        // `CON.` must yield `CON_safe`, never `CON._safe`.
        assert_eq!(
            sanitize_filename_component("CON."),
            Some("CON_safe".to_string())
        );
        // Nothing creatable remains.
        assert_eq!(sanitize_filename_component("..."), None);
        assert_eq!(sanitize_filename_component("   "), None);
        // Mid-name dots and spaces are untouched.
        assert_eq!(
            sanitize_filename_component("a.b c"),
            Some("a.b c".to_string())
        );
    }

    #[test]
    fn sanitize_ntfs_neutralization_is_idempotent() {
        // Every neutralized form must survive a second pass unchanged —
        // otherwise a re-sanitized name keeps drifting (the #1125 idempotence
        // contract applied to the new rules).
        for raw in ["report.txt:hidden", "a<b>c", "report.", "CON.", "Nul:ads"] {
            let once = sanitize_filename_component(raw)
                .unwrap_or_else(|| panic!("'{raw}' must stay usable"));
            let twice = sanitize_filename_component(&once)
                .unwrap_or_else(|| panic!("'{once}' must stay usable"));
            assert_eq!(once, twice, "'{raw}' is not idempotent");
        }
    }

    #[test]
    fn derive_filename_neutralizes_ntfs_hazards_from_content_disposition() {
        // End-to-end over the server-controlled input that actually carries the
        // hazard, not just the primitive.
        let url = url::Url::parse("https://example.com/download").expect("url");
        let derived = derive_filename_from_content_disposition(
            Some(r#"attachment; filename="informe.txt:oculto""#),
            &url,
            "text/plain",
        );
        assert_eq!(derived, "informe.txt_oculto");
        assert!(
            !derived.contains(':'),
            "an ADS colon must never reach the filesystem: {derived}"
        );
    }
}
