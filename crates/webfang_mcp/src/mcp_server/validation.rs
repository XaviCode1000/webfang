//! MCP parameter validation helpers (issue #512, Slice 1)
//!
//! Centralised validation for tool parameter structs. Each helper returns
//! `Result<_, McpError>` using `McpError::invalid_params` so handlers can
//! short-circuit with `?`. The module is framework-agnostic: it knows nothing
//! about the tool router or the request context.
//!
//! Every helper produces an error envelope identical to the one already used
//! by handlers (`McpError::invalid_params(format!(...), Some(field))`), so the
//! slice-2 wiring (`params.validate()?`) is a drop-in replacement.

use rmcp::ErrorData as McpError;
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Max URL length. 8 KiB matches the upper bound recommended by RFC 9110 §5.4
/// for URI references and protects the server from memory DoS via oversize
/// inputs.
pub const MAX_URL_LEN: usize = 8192;

/// Max filesystem path length. 1 KiB is generous for relative paths and
/// protects against accidentally-joined traversal strings.
pub const MAX_PATH_LEN: usize = 1024;

/// Max HTML / markdown / content blob length. 1 MiB protects the server from
/// memory exhaustion via oversize inputs (legitimate pages fit comfortably).
pub const MAX_BLOB_LEN: usize = 1_048_576;

/// Max length of a domain string (RFC 1035 §2.3.4 caps FQDNs at 253 octets).
pub const MAX_DOMAIN_LEN: usize = 253;

/// Build the standard `McpError::invalid_params` envelope used by every
/// handler in this crate. Keeps the field tag and message format identical to
/// the inline construction at `handlers/scraping.rs:34-37`.
fn invalid_params(field: &str, msg: impl Into<String>) -> McpError {
    McpError::invalid_params(msg.into(), Some(Value::String(field.to_string())))
}

/// Validate that `value` parses as an http or https URL.
///
/// Rejects: empty, longer than [`MAX_URL_LEN`], non-http(s) schemes (file://,
/// ftp://, gopher://, data:, javascript:, etc.), and unparseable strings.
/// Returns the parsed URL on success so callers can reuse it without a
/// second parse.
///
/// # Errors
/// Returns `McpError::invalid_params` for any of the rejection reasons above.
pub fn require_http_url(field: &str, value: &str) -> Result<url::Url, McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if value.len() > MAX_URL_LEN {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {MAX_URL_LEN} bytes"),
        ));
    }
    let parsed =
        url::Url::parse(value).map_err(|e| invalid_params(field, format!("invalid URL: {e}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(invalid_params(
            field,
            format!("unsupported scheme '{other}' (only http and https are allowed)"),
        )),
    }
}

/// Validate that `value` is a safe filesystem path: non-empty, ≤
/// [`MAX_PATH_LEN`], relative (no leading `/`, no Windows drive letter), and
/// free of `..` traversal components.
///
/// # Errors
/// Returns `McpError::invalid_params` for empty, oversize, absolute, or
/// `..`-traversal paths.
pub fn require_safe_path(field: &str, value: &str) -> Result<PathBuf, McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
        ));
    }
    let path = Path::new(value);
    // Reject absolute paths. `Path::is_absolute()` is platform-aware (returns
    // false for `C:\Windows` on Unix), so also probe the string for leading
    // slashes, UNC prefixes, and Windows drive letters explicitly.
    if value.starts_with('/') || value.starts_with('\\') {
        return Err(invalid_params(
            field,
            "must be a relative path (no leading '/' or UNC prefix)",
        ));
    }
    if has_windows_drive_prefix(value) {
        return Err(invalid_params(
            field,
            "must be a relative path (no Windows drive letter)",
        ));
    }
    if path.is_absolute() {
        return Err(invalid_params(
            field,
            "must be a relative path (no leading '/' or Windows drive letter)",
        ));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params(
            field,
            "must not contain '..' traversal components",
        ));
    }
    Ok(path.to_path_buf())
}

/// Detect a Windows-style drive-letter prefix (e.g. `C:\foo` or `C:/foo`)
/// regardless of host platform. Case-insensitive letter; separator can be
/// either `\` or `/`.
fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// Validate that `value` is a safe filesystem path: non-empty, ≤
/// [`MAX_PATH_LEN`], absolute paths ALLOWED (leading `/` accepted), and free
/// of `..` traversal components.
///
/// Unlike [`require_safe_path`], this variant accepts absolute paths so
/// tools like `detect_obsidian_vault` can accept `/home/user/vault`. Still
/// rejects `..` traversal and oversize inputs.
///
/// # Errors
/// Returns `McpError::invalid_params` for empty, oversize, or
/// `..`-traversal paths.
pub fn require_safe_path_allow_absolute(field: &str, value: &str) -> Result<PathBuf, McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
        ));
    }
    let path = Path::new(value);
    // Absolute paths are intentionally allowed — Obsidian vaults live at
    // user-supplied absolute locations (issue #590, bug #8).
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params(
            field,
            "must not contain '..' traversal components",
        ));
    }
    Ok(path.to_path_buf())
}

/// Validate that `value` is at most `max_len` characters long.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value.len() > max_len`.
pub fn require_max_len(field: &str, value: &str, max_len: usize) -> Result<(), McpError> {
    if value.len() > max_len {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {max_len} bytes"),
        ));
    }
    Ok(())
}

/// Validate that `value` is non-empty and ≤ [`MAX_PATH_LEN`] characters.
///
/// Use for fields like `filename` and `vault_name` that are joined into paths
/// but are not paths themselves (no traversal check needed).
///
/// # Errors
/// Returns `McpError::invalid_params` if `value` is empty or exceeds
/// [`MAX_PATH_LEN`].
pub fn require_safe_name(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
        ));
    }
    Ok(())
}

/// Validate that `value` is a well-formed bare domain string (e.g. "example.com"
/// or "a.b.c.d.e.example.com"): non-empty, ≤ [`MAX_DOMAIN_LEN`], no path or
/// scheme separators (`/`, `\`, `:`), no whitespace, no `..` segments, and at
/// least one `.`.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value` fails any of the bare-domain
/// rules.
pub fn require_safe_domain(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if value.len() > MAX_DOMAIN_LEN {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {MAX_DOMAIN_LEN} bytes"),
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(invalid_params(field, "must not contain whitespace"));
    }
    if value.contains(['/', '\\', ':']) {
        return Err(invalid_params(
            field,
            "must be a bare domain (no path, scheme, or port separator)",
        ));
    }
    if value.contains("..") {
        return Err(invalid_params(field, "must not contain '..'"));
    }
    if !value.contains('.') {
        return Err(invalid_params(
            field,
            "must contain at least one '.' separator",
        ));
    }
    Ok(())
}

/// Validate a seed host: a bare domain (e.g. "example.com") OR an http(s) URL
/// (e.g. `<https://example.com/path>`). Mirrors the core's
/// `url_validation::normalize_seed_host` acceptance so MCP validation does not
/// over-reject input the core legitimately handles. Rejects empty, whitespace,
/// `..` traversal, and non-http(s) schemes (file://, ftp://, ...).
///
/// # Errors
/// Returns `McpError::invalid_params` if `value` is empty, contains
/// whitespace, `..`, a disallowed scheme, or is neither a bare domain nor an
/// http(s) URL.
pub fn require_safe_seed(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(invalid_params(field, "must not contain whitespace"));
    }
    if value.contains("..") {
        return Err(invalid_params(field, "must not contain '..'"));
    }
    // URL form: require an http(s) scheme. `split_once("://")` distinguishes
    // "https://x" (scheme present) from "example.com" (no "://", bare host).
    if let Some((scheme, _rest)) = value.split_once("://") {
        if scheme != "http" && scheme != "https" {
            return Err(invalid_params(
                field,
                format!("unsupported scheme '{scheme}' (only http/https allowed)"),
            ));
        }
        return Ok(());
    }
    // Bare host form: require a domain shape (at least one '.', no '/' or ':').
    if value.contains(['/', ':']) {
        return Err(invalid_params(
            field,
            "must be a bare domain or http(s) URL",
        ));
    }
    if !value.contains('.') {
        return Err(invalid_params(
            field,
            "must contain at least one '.' separator",
        ));
    }
    Ok(())
}

/// Validate that `value` is non-empty.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value` is empty.
pub fn require_non_empty(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    Ok(())
}

/// Validate that `value` does not exceed `max`.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value > max`.
pub fn require_max_value_u64(field: &str, value: u64, max: u64) -> Result<(), McpError> {
    if value > max {
        return Err(invalid_params(field, format!("must be at most {max}")));
    }
    Ok(())
}

/// Validate that `value >= min`.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value < min`.
pub fn require_min_value_u64(field: &str, value: u64, min: u64) -> Result<(), McpError> {
    if value < min {
        return Err(invalid_params(field, format!("must be at least {min}")));
    }
    Ok(())
}

/// Validate that `value` is a single, flat filename component safe to join
/// onto a base directory.
///
/// Unlike [`require_safe_name`] — which validates only length/emptiness and
/// assumes the value is never used as a path component — this enforces, by
/// structural decomposition, that the value cannot escape its parent directory
/// when joined via `Path::join`. This is the fix for issue #601: a `filename`
/// of `"../escape"` or `"sub/out"` must never reach `std::fs`.
///
/// Decomposition rules (Rust `Path::components`):
/// 1. Exactly **one** component.
/// 2. That component is of kind [`Component::Normal`] (rejects `ParentDir`
///    (`..`), `CurDir` (`.`), `RootDir` (`/`), and `Prefix` (Windows drive)).
/// 3. The component's string representation equals the original `value`
///    byte-for-byte, so platform-specific separator filtering (`/` and `\`)
///    cannot slip through.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value` is empty, oversize, or not a
/// single flat `Normal` component.
pub fn require_safe_filename(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
        ));
    }
    // Reject separators explicitly so the rule is platform-independent: on
    // Unix `\` is a legal filename byte, but we must never let a
    // platform-specific component parse mask a real traversal risk (issue
    // #601). `Path::components` below is the structural backstop.
    if value.contains(['/', '\\']) {
        return Err(invalid_params(
            field,
            "must be a single flat filename (no '/' or '\\' separators)",
        ));
    }
    let mut components = Path::new(value).components();
    let Some(component) = components.next() else {
        return Err(invalid_params(field, "must be a single filename component"));
    };
    if components.next().is_some() {
        return Err(invalid_params(
            field,
            "must not contain path separators or directory components",
        ));
    }
    if !matches!(component, Component::Normal(_)) {
        return Err(invalid_params(
            field,
            "must be a single flat filename (no '.', '..', '/', or drive prefix)",
        ));
    }
    if component.as_os_str().to_string_lossy() != value {
        return Err(invalid_params(
            field,
            "must be a single flat filename (no embedded separators)",
        ));
    }
    Ok(())
}

/// A filename that is safe to join onto a base directory — guaranteed by
/// construction (issue #601).
///
/// A value of this type can ONLY be produced by [`SanitizedFilename::try_from`]
/// (or [`std::str::FromStr`]), which rejects anything that is not a single flat
/// [`Component::Normal`]. Handlers must thread this type across layers instead
/// of a raw `String`, so an unvalidated `..` can never reach `std::fs`.
///
/// This makes the "invalid state unrepresentable": the only way to obtain a
/// `SanitizedFilename` is through validation, and the validation is exhaustive
/// at the boundary. There is no `unsafe` escape hatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SanitizedFilename(String);

impl SanitizedFilename {
    /// Borrow the validated, flat filename.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SanitizedFilename {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for SanitizedFilename {
    type Err = McpError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        require_safe_filename("filename", s).map(|()| SanitizedFilename(s.to_string()))
    }
}

impl TryFrom<&str> for SanitizedFilename {
    type Error = McpError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        std::str::FromStr::from_str(value)
    }
}

/// Validate that `value` does not exceed `max`.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value > max`.
pub fn require_max_value_u16(field: &str, value: u16, max: u16) -> Result<(), McpError> {
    if value > max {
        return Err(invalid_params(field, format!("must be at most {max}")));
    }
    Ok(())
}

/// Validate that `value` (case-insensitive) is one of `options`.
///
/// # Errors
/// Returns `McpError::invalid_params` if `value` does not match any option
/// (case-insensitive).
pub fn require_one_of(field: &str, value: &str, options: &[&str]) -> Result<(), McpError> {
    let lower = value.to_ascii_lowercase();
    if options.iter().any(|o| o.eq_ignore_ascii_case(&lower)) {
        Ok(())
    } else {
        Err(invalid_params(
            field,
            format!("must be one of: {} (got '{value}')", options.join(", ")),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn require_safe_path_allow_absolute_accepts_absolute() {
        // Bug #8 regression: absolute paths MUST be accepted (issue #590).
        let result = require_safe_path_allow_absolute("vault_path", "/home/user/vault");
        assert!(result.is_ok(), "absolute path must be accepted: {result:?}");
        assert_eq!(result.unwrap().to_string_lossy(), "/home/user/vault");
    }

    #[test]
    fn require_safe_path_allow_absolute_rejects_traversal() {
        // `..` traversal must still be rejected even for absolute paths.
        let result = require_safe_path_allow_absolute("vault_path", "/home/user/../etc/passwd");
        assert!(result.is_err(), "traversal must be rejected: {result:?}");
    }

    #[test]
    fn require_safe_path_allow_absolute_rejects_empty() {
        let result = require_safe_path_allow_absolute("vault_path", "");
        assert!(result.is_err(), "empty path must be rejected: {result:?}");
    }

    #[test]
    fn require_safe_path_allow_absolute_accepts_relative() {
        // Relative paths should still work (no regression).
        let result = require_safe_path_allow_absolute("vault_path", "my-vault");
        assert!(result.is_ok(), "relative path must be accepted: {result:?}");
    }

    // --- require_safe_filename (issue #601) ---------------------------------

    #[test]
    fn require_safe_filename_accepts_flat_name() {
        assert!(require_safe_filename("filename", "doc").is_ok());
        assert!(require_safe_filename("filename", "report-2026.json").is_ok());
        assert!(require_safe_filename("filename", "a.b.c").is_ok());
    }

    #[test]
    fn require_safe_filename_rejects_parent_traversal() {
        // The exact payload from issue #601.
        assert!(require_safe_filename("filename", "../escape").is_err());
        assert!(require_safe_filename("filename", "sub/../escape").is_err());
        assert!(require_safe_filename("filename", "..").is_err());
    }

    #[test]
    fn require_safe_filename_rejects_subdirectory() {
        // `sub/out` must not silently create nested directories.
        assert!(require_safe_filename("filename", "sub/out").is_err());
        assert!(require_safe_filename("filename", "a/b/c").is_err());
    }

    #[test]
    fn require_safe_filename_rejects_separators_and_root() {
        assert!(require_safe_filename("filename", "/etc/passwd").is_err());
        assert!(require_safe_filename("filename", ".\\windows").is_err());
        assert!(require_safe_filename("filename", "").is_err());
        assert!(require_safe_filename("filename", ".").is_err());
    }

    #[test]
    fn sanitized_filename_newtype_rejects_traversal() {
        assert!("sub/out".parse::<SanitizedFilename>().is_err());
        assert!("..".parse::<SanitizedFilename>().is_err());
        let ok = "doc".parse::<SanitizedFilename>().expect("flat name valid");
        assert_eq!(ok.as_str(), "doc");
    }
}
