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
