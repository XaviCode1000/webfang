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
//!
//! Since EC-08 (issue #1613) that envelope's `data` is a structured OBJECT
//! (`{"field": …, "reason": <slug>}`) rather than a bare string, so a caller
//! can tell WHICH field was wrong and WHY without parsing prose. The seven
//! stable slugs are `validation::REASON_*` (documented on the
//! `invalid_params_with_reason` builder below, which is module-private).

use rmcp::ErrorData as McpError;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use webfang_core::domain::crawler_port::filename::is_windows_reserved;

/// Max URL length. 8 KiB matches the upper bound recommended by RFC 9110 §5.4
/// for URI references and protects the server from memory DoS via oversize
/// inputs.
pub const MAX_URL_LEN: usize = 8192;

/// Max filesystem path length. 1 KiB is generous for relative paths and
/// protects against accidentally-joined traversal strings.
pub const MAX_PATH_LEN: usize = 1024;

/// Max bytes per single filename component (issue #1608, XP-P-07 partial).
/// ext4 caps at 255; we target the lowest common denominator so a name
/// accepted here is creatable on every supported filesystem. Long-name
/// mitigation (`\\?\` extended-length prefixes, manifests) is explicitly out
/// of scope.
const MAX_COMPONENT_BYTES: usize = 255;

/// Max HTML / markdown / content blob length. 1 MiB protects the server from
/// memory exhaustion via oversize inputs (legitimate pages fit comfortably).
pub const MAX_BLOB_LEN: usize = 1_048_576;

/// Max length of a domain string (RFC 1035 §2.3.4 caps FQDNs at 253 octets).
pub const MAX_DOMAIN_LEN: usize = 253;

/// The required value is empty — fill it in.
pub const REASON_EMPTY: &str = "empty";
/// The value exceeds a byte cap — shorten it.
pub const REASON_TOO_LONG: &str = "too_long";
/// The value is present but unparseable — fix its shape.
pub const REASON_MALFORMED: &str = "malformed";
/// The URL scheme is not `http`/`https` — use one of those.
pub const REASON_UNSUPPORTED_SCHEME: &str = "unsupported_scheme";
/// The path is rejected for any structural reason (leading `/`, UNC prefix,
/// Windows drive letter, `..` traversal, non-flat filename, Windows reserved
/// name, Windows-invalid character, trailing `.`/space) — use a safe relative
/// path and a plain filename.
pub const REASON_PATH_NOT_ALLOWED: &str = "path_not_allowed";
/// A numeric value is outside its inclusive bounds — clamp it.
pub const REASON_OUT_OF_RANGE: &str = "out_of_range";
/// An enum-ish value is not in the allowed list — pick a listed value.
pub const REASON_NOT_IN_ALLOWED_SET: &str = "not_in_allowed_set";

/// Build the standard `McpError::invalid_params` envelope used by every
/// handler in this crate, WITH a stable [`REASON_*`] slug. `data` is
/// `{"field": "<field>", "reason": "<slug>"}`.
///
/// Every rejection branch in the crate funnels through here, which is what
/// makes the taxonomy exhaustive: a caller can always branch on `data.reason`
/// instead of parsing prose. The sibling SSRF channel
/// ([`crate::mcp_server::ssrf`]) attaches the same `data.reason` key without a
/// `field` — an SSRF refusal is not a bad field but a policy decision or a
/// server-side DNS fault — so one reader handles both.
///
/// # The reason taxonomy is a STABLE contract (EC-08, issue #1613)
///
/// `error.data` used to be a bare JSON string naming the field, and every
/// rejection shared the JSON-RPC code `-32602` — an agent could tell which
/// field was wrong but never why, so it had to parse prose. `data` is now an
/// object and `data.reason` is one of exactly SEVEN coarse slugs:
///
/// | slug | meaning | what the caller should do |
/// | :--- | :--- | :--- |
/// | [`REASON_EMPTY`] | required value is empty | fill it in |
/// | [`REASON_TOO_LONG`] | exceeds a byte cap | shorten it |
/// | [`REASON_MALFORMED`] | present but unparseable | fix its shape |
/// | [`REASON_UNSUPPORTED_SCHEME`] | not `http`/`https` | use http or https |
/// | [`REASON_PATH_NOT_ALLOWED`] | any path rejection (absolute, UNC, drive letter, `..`, non-flat filename, reserved name, illegal char, trailing `.`/space) | use a safe relative path |
/// | [`REASON_OUT_OF_RANGE`] | numeric bound | clamp it |
/// | [`REASON_NOT_IN_ALLOWED_SET`] | not in the allowed list | pick a listed value |
///
/// **Coarse is deliberate, and the set is load-bearing.** A slug's job is to
/// tell an agent what to DO, not to restate the message; seven stable values
/// beat forty brittle ones. Downstream tooling branches on these strings, so
/// do not rename one, add an eighth, or change a mapping to a different slug
/// without a matching change to the documented contract in
/// `docs/src/mcp-error-contract.md`. Message text remains the human-readable
/// half and is NOT part of this contract (the module mixes English and one
/// Spanish island by design, #1613).
///
/// There is deliberately no reason-less variant: a caller that cannot name a
/// slug should pick the closest one rather than ship an absent reason, because
/// a consumer branching on `data.reason` must be able to rely on it being
/// there. Every rejection in the crate therefore carries one.
pub(crate) fn invalid_params_with_reason(
    field: &str,
    msg: impl Into<String>,
    reason: &str,
) -> McpError {
    McpError::invalid_params(
        msg.into(),
        Some(json!({ "field": field, "reason": reason })),
    )
}

/// Read the `reason` slug out of an `invalid_params` `data` payload.
///
/// Tolerates a `data` that is `None`, a non-object (the pre-EC-08 bare
/// string), or an object without a `reason` key — every one of those yields
/// `None` rather than panicking, because `data` is sender-defined and this
/// helper sits on a success-shaped diagnostic path.
pub(crate) fn reason_of(data: Option<&Value>) -> Option<&str> {
    data?.get("reason")?.as_str()
}

/// Validate that `value` parses as an http or https URL.
///
/// Rejects: empty, longer than [`MAX_URL_LEN`], non-http(s) schemes (file://,
/// ftp://, gopher://, data:, javascript:, etc.), and unparseable strings.
/// Returns the parsed URL on success so callers can reuse it without a
/// second parse.
///
/// # Errors
/// Returns `McpError::invalid_params` for any of the rejection reasons above,
/// carrying [`REASON_EMPTY`], [`REASON_TOO_LONG`], [`REASON_MALFORMED`], or
/// [`REASON_UNSUPPORTED_SCHEME`] respectively.
pub fn require_http_url(field: &str, value: &str) -> Result<url::Url, McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_URL_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_URL_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    let parsed = url::Url::parse(value).map_err(|e| {
        invalid_params_with_reason(field, format!("invalid URL: {e}"), REASON_MALFORMED)
    })?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(invalid_params_with_reason(
            field,
            format!("unsupported scheme '{other}' (only http and https are allowed)"),
            REASON_UNSUPPORTED_SCHEME,
        )),
    }
}

/// Validate that `value` is a safe filesystem path: non-empty, ≤
/// [`MAX_PATH_LEN`], relative (no leading `/`, no Windows drive letter), and
/// free of `..` traversal components.
///
/// # Errors
/// Returns `McpError::invalid_params` for empty, oversize, absolute, or
/// `..`-traversal paths — [`REASON_EMPTY`], [`REASON_TOO_LONG`], or
/// [`REASON_PATH_NOT_ALLOWED`] respectively. Every path-level rule shares the
/// one `path_not_allowed` slug on purpose: what the caller must DO (switch to a
/// safe relative path) is identical for all of them, and the message carries
/// the specifics.
pub fn require_safe_path(field: &str, value: &str) -> Result<PathBuf, McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    let path = Path::new(value);
    // Reject absolute paths. `Path::is_absolute()` is platform-aware (returns
    // false for `C:\Windows` on Unix), so also probe the string for leading
    // slashes, UNC prefixes, and Windows drive letters explicitly.
    if value.starts_with('/') || value.starts_with('\\') {
        return Err(invalid_params_with_reason(
            field,
            "must be a relative path (no leading '/' or UNC prefix)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if has_windows_drive_prefix(value) {
        return Err(invalid_params_with_reason(
            field,
            "must be a relative path (no Windows drive letter)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if path.is_absolute() {
        return Err(invalid_params_with_reason(
            field,
            "must be a relative path (no leading '/' or Windows drive letter)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..' traversal components",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // Per-component filename hardening (issue #1608): the relative-only
    // contract means no drive prefix is legitimate anywhere in the value.
    if let Some(reason) = normal_components(path).find_map(|c| filename_component_error(&c, false))
    {
        return Err(invalid_params_with_reason(
            field,
            reason,
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    Ok(path.to_path_buf())
}

/// The `Normal` components of `path` as lossy strings — the only components
/// the filename-level checks apply to (structural kinds are handled by the
/// callers' own rules).
fn normal_components(path: &Path) -> impl Iterator<Item = std::borrow::Cow<'_, str>> {
    path.components().filter_map(|c| match c {
        Component::Normal(os) => Some(os.to_string_lossy()),
        _ => None,
    })
}

/// Detect a Windows-style drive-letter prefix (letter + `:`) regardless of
/// host platform. Case-insensitive letter; NO separator requirement — the
/// drive-relative `C:foo` form (relative to the current directory ON drive
/// C:, an escape on Windows hosts) must be rejected by
/// [`require_safe_path`]'s relative-only contract just like `C:\foo` (#1588).
/// `require_safe_path_allow_absolute` deliberately does not call this: it
/// accepts absolute Windows paths (`C:\vault`).
fn has_windows_drive_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Filename-level hardening shared by [`require_safe_filename`] and the
/// per-component extension of [`require_safe_path`] /
/// [`require_safe_path_allow_absolute`] (issue #1608).
///
/// Returns the rejection MESSAGE only — every caller attaches
/// [`REASON_PATH_NOT_ALLOWED`], because the caller's remedy ("use a plain
/// filename inside a safe relative path") is the same for all of them, while
/// the message carries the specific rule that fired.
///
/// Checks one path component (no separators can occur inside a
/// `Component::Normal`) and returns the Spanish rejection reason for the
/// first violated rule, or `None` when the component is safe:
///
/// * control characters (Unicode `Cc`: NUL, C0, DEL, C1) — rejected;
/// * `:` anywhere (NTFS alternate-data-stream hazard) — rejected on every
///   platform for cross-platform consistency (XP-P-05);
/// * the remaining Windows-invalid set `< > " | ? *` (`/` and `\` are
///   handled structurally by the callers);
/// * Windows reserved device names via the shared stem-aware
///   `is_windows_reserved` (`CON`, `con.txt`, ... — XP-P-04);
/// * trailing `.` or trailing space (Windows strips both silently — XP-P-06;
///   rejected, never trimmed, so callers see exactly what was asked);
/// * component length over [`MAX_COMPONENT_BYTES`] (XP-P-07 partial).
///
/// `allow_drive_prefix` tolerates a leading `X:` (drive letter) in the
/// component: `require_safe_path_allow_absolute` accepts absolute Windows
/// paths like `C:\vault` (issue #590), whose drive colon is a separator, not
/// an ADS hazard. Flat filenames and relative-only paths pass `false` — a
/// filename can never be a drive, and a relative path must never contain one.
fn filename_component_error(component: &str, allow_drive_prefix: bool) -> Option<String> {
    if component.chars().any(char::is_control) {
        return Some("contiene caracteres de control no válidos".to_string());
    }
    let probe = if allow_drive_prefix && has_windows_drive_prefix(component) {
        &component[2..]
    } else {
        component
    };
    if probe.contains(':') {
        return Some("no debe contener ':' (riesgo de flujos alternativos NTFS)".to_string());
    }
    if component
        .chars()
        .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Some("contiene caracteres no permitidos en Windows: < > \" | ? *".to_string());
    }
    if is_windows_reserved(component) {
        return Some(
            "usa un nombre reservado de Windows (CON, PRN, AUX, NUL, COM1-9, LPT1-9)".to_string(),
        );
    }
    if component.ends_with('.') || component.ends_with(' ') {
        return Some("no debe terminar en '.' ni en espacio".to_string());
    }
    if component.len() > MAX_COMPONENT_BYTES {
        return Some(format!(
            "supera el límite de {MAX_COMPONENT_BYTES} bytes por componente"
        ));
    }
    None
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
/// `..`-traversal paths — [`REASON_EMPTY`], [`REASON_TOO_LONG`], or
/// [`REASON_PATH_NOT_ALLOWED`] respectively.
pub fn require_safe_path_allow_absolute(field: &str, value: &str) -> Result<PathBuf, McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    let path = Path::new(value);
    // Absolute paths are intentionally allowed — Obsidian vaults live at
    // user-supplied absolute locations (issue #590, bug #8).
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..' traversal components",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // Per-component filename hardening (issue #1608). The drive-prefix
    // exception keeps absolute Windows paths (`C:\vault`, #590) valid: the
    // drive colon is a separator there, not an ADS hazard.
    if let Some(reason) = normal_components(path).find_map(|c| filename_component_error(&c, true)) {
        return Err(invalid_params_with_reason(
            field,
            reason,
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    Ok(path.to_path_buf())
}

/// Validate that `value` is at most `max_len` characters long.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_TOO_LONG`] if
/// `value.len() > max_len`.
pub fn require_max_len(field: &str, value: &str, max_len: usize) -> Result<(), McpError> {
    if value.len() > max_len {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {max_len} bytes"),
            REASON_TOO_LONG,
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
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`] or
/// [`REASON_TOO_LONG`] if `value` is empty or exceeds [`MAX_PATH_LEN`].
pub fn require_safe_name(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
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
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`],
/// [`REASON_TOO_LONG`], [`REASON_MALFORMED`] (whitespace, stray separator,
/// no `.` separator) or [`REASON_PATH_NOT_ALLOWED`] (a `..` segment) if
/// `value` fails any of the bare-domain rules.
pub fn require_safe_domain(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_DOMAIN_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_DOMAIN_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain whitespace",
            REASON_MALFORMED,
        ));
    }
    if value.contains(['/', '\\', ':']) {
        return Err(invalid_params_with_reason(
            field,
            "must be a bare domain (no path, scheme, or port separator)",
            REASON_MALFORMED,
        ));
    }
    if value.contains("..") {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..'",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if !value.contains('.') {
        return Err(invalid_params_with_reason(
            field,
            "must contain at least one '.' separator",
            REASON_MALFORMED,
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
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`],
/// [`REASON_MALFORMED`] (whitespace, neither bare domain nor URL, no `.`
/// separator), [`REASON_PATH_NOT_ALLOWED`] (a `..` segment) or
/// [`REASON_UNSUPPORTED_SCHEME`] (a non-http(s) URL scheme) if `value` is
/// empty, contains whitespace, `..`, a disallowed scheme, or is neither a bare
/// domain nor an http(s) URL.
pub fn require_safe_seed(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain whitespace",
            REASON_MALFORMED,
        ));
    }
    if value.contains("..") {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..'",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // URL form: require an http(s) scheme. `split_once("://")` distinguishes
    // "https://x" (scheme present) from "example.com" (no "://", bare host).
    if let Some((scheme, _rest)) = value.split_once("://") {
        if scheme != "http" && scheme != "https" {
            return Err(invalid_params_with_reason(
                field,
                format!("unsupported scheme '{scheme}' (only http/https allowed)"),
                REASON_UNSUPPORTED_SCHEME,
            ));
        }
        return Ok(());
    }
    // Bare host form: require a domain shape (at least one '.', no '/' or ':').
    if value.contains(['/', ':']) {
        return Err(invalid_params_with_reason(
            field,
            "must be a bare domain or http(s) URL",
            REASON_MALFORMED,
        ));
    }
    if !value.contains('.') {
        return Err(invalid_params_with_reason(
            field,
            "must contain at least one '.' separator",
            REASON_MALFORMED,
        ));
    }
    Ok(())
}

/// Validate that `value` is non-empty.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`] if `value` is
/// empty.
pub fn require_non_empty(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    Ok(())
}

/// Validate that `value` does not exceed `max`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_OUT_OF_RANGE`] if
/// `value > max`.
pub fn require_max_value_u64(field: &str, value: u64, max: u64) -> Result<(), McpError> {
    if value > max {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at most {max}"),
            REASON_OUT_OF_RANGE,
        ));
    }
    Ok(())
}

/// Validate that `min <= value <= max`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_OUT_OF_RANGE`] if `value`
/// is outside the inclusive range.
pub fn require_range_u64(field: &str, value: u64, min: u64, max: u64) -> Result<(), McpError> {
    if value < min {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at least {min}"),
            REASON_OUT_OF_RANGE,
        ));
    }
    if value > max {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at most {max}"),
            REASON_OUT_OF_RANGE,
        ));
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
/// Cross-platform filename hardening (issue #1608), via the private
/// `filename_component_error` helper with NO drive-prefix exception (a flat
/// filename can never be a drive): control characters (NUL included), `:`
/// anywhere (NTFS ADS hazard), the Windows-invalid set `< > " | ? *`,
/// Windows reserved device names (`CON`, `con.txt`, ... — stem-aware), and
/// trailing `.` / trailing space are all rejected — never trimmed, so
/// callers see exactly what was asked. Per-component cap: 255 bytes
/// (XP-P-07 partial; `\\?\` mitigation is out of scope).
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_EMPTY`],
/// [`REASON_TOO_LONG`], or [`REASON_PATH_NOT_ALLOWED`] if `value` is empty,
/// oversize, not a single flat `Normal` component, or violates any
/// filename-hardening rule.
pub fn require_safe_filename(field: &str, value: &str) -> Result<(), McpError> {
    if value.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if value.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    // Reject separators explicitly so the rule is platform-independent: on
    // Unix `\` is a legal filename byte, but we must never let a
    // platform-specific component parse mask a real traversal risk (issue
    // #601). `Path::components` below is the structural backstop.
    if value.contains(['/', '\\']) {
        return Err(invalid_params_with_reason(
            field,
            "must be a single flat filename (no '/' or '\\' separators)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    let mut components = Path::new(value).components();
    let Some(component) = components.next() else {
        return Err(invalid_params_with_reason(
            field,
            "must be a single filename component",
            REASON_PATH_NOT_ALLOWED,
        ));
    };
    if components.next().is_some() {
        return Err(invalid_params_with_reason(
            field,
            "must not contain path separators or directory components",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if !matches!(component, Component::Normal(_)) {
        return Err(invalid_params_with_reason(
            field,
            "must be a single flat filename (no '.', '..', '/', or drive prefix)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if component.as_os_str().to_string_lossy() != value {
        return Err(invalid_params_with_reason(
            field,
            "must be a single flat filename (no embedded separators)",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    // Cross-platform hardening (issue #1608). `false`: a flat filename can
    // never carry a drive prefix, so every `:` is rejected.
    if let Some(reason) = filename_component_error(value, false) {
        return Err(invalid_params_with_reason(
            field,
            reason,
            REASON_PATH_NOT_ALLOWED,
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
/// Returns `McpError::invalid_params` with [`REASON_OUT_OF_RANGE`] if
/// `value > max`.
pub fn require_max_value_u16(field: &str, value: u16, max: u16) -> Result<(), McpError> {
    if value > max {
        return Err(invalid_params_with_reason(
            field,
            format!("must be at most {max}"),
            REASON_OUT_OF_RANGE,
        ));
    }
    Ok(())
}

/// Validate that `value` (case-insensitive) is one of `options`.
///
/// # Errors
/// Returns `McpError::invalid_params` with [`REASON_NOT_IN_ALLOWED_SET`] if
/// `value` does not match any option (case-insensitive).
pub fn require_one_of(field: &str, value: &str, options: &[&str]) -> Result<(), McpError> {
    let lower = value.to_ascii_lowercase();
    if options.iter().any(|o| o.eq_ignore_ascii_case(&lower)) {
        Ok(())
    } else {
        Err(invalid_params_with_reason(
            field,
            format!("must be one of: {} (got '{value}')", options.join(", ")),
            REASON_NOT_IN_ALLOWED_SET,
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

    // --- require_safe_path drive-relative probe (#1588) --------------------

    #[test]
    fn require_safe_path_rejects_drive_relative_without_separator() {
        // #1588: `C:foo` is drive-relative (relative to the current directory
        // ON the drive) — an escape on Windows hosts, never a safe relative
        // path. The old separator-requiring probe let it through on every host.
        for value in ["C:foo", "c:foo", "C:", "D:exports/2026"] {
            let err = require_safe_path("file_path", value).unwrap_err();
            assert!(
                matches!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS),
                "{value} must be rejected as drive-relative"
            );
        }
    }

    #[test]
    fn require_safe_path_allow_absolute_keeps_accepting_absolute_windows_paths() {
        // The drive-prefix change must NOT leak into the allow-absolute
        // variant: `C:\vault` stays valid for `detect_obsidian_vault` (#590).
        let result = require_safe_path_allow_absolute("vault_path", "C:\\vault");
        assert!(
            result.is_ok(),
            "absolute Windows path must be accepted: {result:?}"
        );
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

    // --- cross-platform filename hardening (issue #1608) --------------------

    #[test]
    fn require_safe_filename_accepts_plain_cross_platform_names() {
        // Sanity: the hardening must not reject ordinary names.
        assert!(require_safe_filename("filename", "documento final").is_ok());
        assert!(require_safe_filename("filename", "report.2026-09-26.json").is_ok());
        assert!(require_safe_filename("filename", "naïve-file_λ").is_ok());
    }

    #[test]
    fn require_safe_filename_rejects_control_characters() {
        // NUL, C0 escapes, DEL and C1 are all Unicode Cc — XP-P-01-adjacent.
        assert!(require_safe_filename("filename", "doc\u{0}ument").is_err());
        assert!(require_safe_filename("filename", "doc\u{1b}[31m").is_err());
        assert!(require_safe_filename("filename", "doc\u{7f}").is_err());
        assert!(require_safe_filename("filename", "doc\u{85}").is_err());
    }

    #[test]
    fn require_safe_filename_rejects_windows_reserved_names() {
        // XP-P-04: stem-aware, case-insensitive — `CON.txt` is as unusable on
        // Windows as `CON`.
        for value in [
            "CON",
            "con",
            "Con",
            "CON.txt",
            "nul.tar.gz",
            "PRN",
            "AUX",
            "COM1",
            "lpt9",
        ] {
            assert!(
                require_safe_filename("filename", value).is_err(),
                "'{value}' must be rejected as a Windows reserved name"
            );
        }
        // Lookalikes that are NOT reserved stay accepted.
        assert!(require_safe_filename("filename", "console").is_ok());
        assert!(require_safe_filename("filename", "com10").is_ok());
    }

    #[test]
    fn require_safe_filename_rejects_colon_ads_hazard() {
        // XP-P-05: any `:` in a flat filename is an NTFS alternate-data-stream
        // hazard; rejected on every host for cross-platform consistency.
        assert!(require_safe_filename("filename", "a:b").is_err());
        assert!(require_safe_filename("filename", "stream.txt:ads").is_err());
        // A drive prefix is NOT a filename exception either.
        assert!(require_safe_filename("filename", "C:").is_err());
    }

    #[test]
    fn require_safe_filename_rejects_trailing_dot_or_space() {
        // XP-P-06: rejected as-is — never silently trimmed.
        assert!(require_safe_filename("filename", "documento final.").is_err());
        assert!(require_safe_filename("filename", "documento final ").is_err());
        // Mid-name dots/spaces stay fine.
        assert!(require_safe_filename("filename", "a.b c").is_ok());
    }

    #[test]
    fn require_safe_filename_rejects_windows_invalid_charset() {
        for value in ["a<b", "a>b", "a\"b", "a|b", "a?b", "a*b"] {
            assert!(
                require_safe_filename("filename", value).is_err(),
                "'{value}' must be rejected as Windows-invalid"
            );
        }
    }

    #[test]
    fn require_safe_filename_rejects_oversize_component() {
        // XP-P-07 partial: a single component over 255 bytes is rejected even
        // though the whole value is still under MAX_PATH_LEN.
        let long = "a".repeat(256);
        assert!(
            long.len() < MAX_PATH_LEN,
            "fixture must be under the 1 KiB cap"
        );
        assert!(require_safe_filename("filename", &long).is_err());
        let ok = "a".repeat(255);
        assert!(require_safe_filename("filename", &ok).is_ok());
    }

    #[test]
    fn require_safe_path_applies_component_checks() {
        // Issue #1608: the same filename-level rules per path component.
        assert!(require_safe_path("file_path", "notes/CON.md").is_err());
        assert!(require_safe_path("file_path", "notes/a:b.md").is_err());
        assert!(require_safe_path("file_path", "notes/doc.. ").is_err());
        assert!(require_safe_path("file_path", "notes/trailing.").is_err());
        assert!(require_safe_path("file_path", "notes/a<b.md").is_err());
        // Benign nested paths stay accepted.
        assert!(require_safe_path("file_path", "notes/2026/09/doc.md").is_ok());
    }

    #[test]
    fn require_safe_path_allow_absolute_applies_component_checks() {
        assert!(require_safe_path_allow_absolute("vault_path", "/vault/CON.md").is_err());
        assert!(require_safe_path_allow_absolute("vault_path", "/vault/a|b.md").is_err());
        // The #590 contract survives: absolute Windows paths with a drive
        // prefix stay valid — the drive colon is a separator, not an ADS.
        assert!(require_safe_path_allow_absolute("vault_path", "C:\\vault").is_ok());
    }

    // --- EC-08 reason-slug contract (issue #1613) ---------------------------
    //
    // The transport-level proof lives in
    // `tests/mcp_validation_reason_code_test.rs` (a live server, real
    // JSON-RPC `error.data`). These cover what the transport cannot reach:
    // the four slugs no tool argument can provoke, the branches behind a
    // typed boundary, and the properties that make the taxonomy load-bearing
    // — the closed 7-value set, the `data` SHAPE, and the fact that adding
    // an 8th slug is a visible act.
    // -----------------------------------------------------------------------

    /// Every slug the module can emit.
    const ALL_REASONS: [&str; 7] = [
        REASON_EMPTY,
        REASON_TOO_LONG,
        REASON_MALFORMED,
        REASON_UNSUPPORTED_SCHEME,
        REASON_PATH_NOT_ALLOWED,
        REASON_OUT_OF_RANGE,
        REASON_NOT_IN_ALLOWED_SET,
    ];

    /// The `(field, reason)` pair carried by a rejection.
    fn tag(err: &McpError) -> (String, Option<String>) {
        let data = err.data.as_ref().expect("every rejection carries `data`");
        assert!(
            data.is_object(),
            "`data` must be a JSON OBJECT now, not a bare string: {data}"
        );
        (
            data.get("field")
                .and_then(Value::as_str)
                .expect("`data.field` must be the offending field")
                .to_string(),
            data.get("reason")
                .and_then(Value::as_str)
                .map(str::to_string),
        )
    }

    #[test]
    fn reason_slugs_are_distinct_and_the_set_is_closed() {
        for (i, a) in ALL_REASONS.iter().enumerate() {
            for b in &ALL_REASONS[i + 1..] {
                assert_ne!(a, b, "two rules must not share a reason slug");
            }
        }
        // The taxonomy is a wire contract: a typo here is a slug no caller
        // branches on. This assertion is the tripwire for that.
        assert_eq!(ALL_REASONS.len(), 7, "the taxonomy has exactly 7 slugs");
    }

    #[test]
    fn every_rejection_helper_emits_a_slug_from_the_closed_set() {
        let errs = [
            require_http_url("url", "").unwrap_err(),
            require_http_url("url", &"x".repeat(MAX_URL_LEN + 1)).unwrap_err(),
            require_http_url("url", "not a url").unwrap_err(),
            require_http_url("url", "ftp://example.com").unwrap_err(),
            require_safe_path("file_path", "").unwrap_err(),
            require_safe_path("file_path", &"x".repeat(MAX_PATH_LEN + 1)).unwrap_err(),
            require_safe_path("file_path", "/etc/passwd").unwrap_err(),
            require_safe_path("file_path", "notes/../escape").unwrap_err(),
            require_safe_path("file_path", "notes/CON.md").unwrap_err(),
            require_safe_path_allow_absolute("vault_path", "").unwrap_err(),
            require_safe_path_allow_absolute("vault_path", "v/../x").unwrap_err(),
            require_max_len("html", "xx", 1).unwrap_err(),
            require_safe_name("vault_name", "").unwrap_err(),
            require_safe_domain("base_domain", "").unwrap_err(),
            require_safe_domain("base_domain", "exa mple.com").unwrap_err(),
            require_safe_domain("base_domain", "example.com/../x").unwrap_err(),
            require_safe_seed("seed_domain", "").unwrap_err(),
            require_safe_seed("seed_domain", "example.com/x").unwrap_err(),
            require_safe_seed("seed_domain", "example.com/../x").unwrap_err(),
            require_safe_seed("seed_domain", "ftp://example.com").unwrap_err(),
            require_non_empty("query", "").unwrap_err(),
            require_max_value_u64("urls", 2, 1).unwrap_err(),
            require_range_u64("concurrency", 0, 1, 5).unwrap_err(),
            require_max_value_u16("retries", 9, 3).unwrap_err(),
            require_safe_filename("filename", "").unwrap_err(),
            require_safe_filename("filename", "../escape").unwrap_err(),
            require_safe_filename("filename", "CON").unwrap_err(),
            require_one_of("content_format", "yaml", &["jsonl", "vector"]).unwrap_err(),
        ];
        for err in &errs {
            let (_, reason) = tag(err);
            let slug = reason.expect("every helper rejection must carry a reason slug");
            assert!(
                ALL_REASONS.contains(&slug.as_str()),
                "'{slug}' is outside the documented 7-value taxonomy: {err:?}"
            );
        }
    }

    #[test]
    fn each_helper_emits_the_slug_its_rule_means() {
        // One representative per slug, so a wrong MAPPING (not a wrong shape)
        // is caught: each of these asserts the specific remediation the slug
        // promises.
        let cases: [(McpError, &str); 7] = [
            (require_http_url("url", "").unwrap_err(), REASON_EMPTY),
            (
                require_max_len("html", "xx", 1).unwrap_err(),
                REASON_TOO_LONG,
            ),
            (
                require_http_url("url", "http://").unwrap_err(),
                REASON_MALFORMED,
            ),
            (
                require_http_url("url", "ftp://example.com").unwrap_err(),
                REASON_UNSUPPORTED_SCHEME,
            ),
            (
                require_safe_path("file_path", "notes/../x").unwrap_err(),
                REASON_PATH_NOT_ALLOWED,
            ),
            (
                require_max_value_u64("max_depth", 11, 10).unwrap_err(),
                REASON_OUT_OF_RANGE,
            ),
            (
                require_one_of("content_format", "yaml", &["jsonl"]).unwrap_err(),
                REASON_NOT_IN_ALLOWED_SET,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(
                tag(&err).1.as_deref(),
                Some(expected),
                "wrong slug for: {err:?}"
            );
        }
    }

    #[test]
    fn data_carries_the_offending_field_beside_the_slug() {
        // The field tag is the information the old bare-string `data` carried;
        // EC-08 keeps it so nothing is lost in the restructure.
        for (err, field) in [
            (
                require_http_url("url", "ftp://x.example").unwrap_err(),
                "url",
            ),
            (require_max_len("html", "xx", 1).unwrap_err(), "html"),
            (
                require_safe_filename("filename", "..").unwrap_err(),
                "filename",
            ),
        ] {
            assert_eq!(tag(&err).0, field, "wrong field tag for: {err:?}");
        }
    }

    #[test]
    fn filename_component_messages_are_unchanged_and_tagged_path_not_allowed() {
        // The Spanish island stays byte-for-byte what it was (no reword, no
        // translate — #1613 documents the language contract, it does not unify
        // it); only the machine-readable half is added.
        let err = require_safe_filename("filename", "CON").unwrap_err();
        assert_eq!(
            err.message,
            "usa un nombre reservado de Windows (CON, PRN, AUX, NUL, COM1-9, LPT1-9)"
        );
        assert_eq!(tag(&err).1.as_deref(), Some(REASON_PATH_NOT_ALLOWED));

        // `a:b` parses as a drive-relative path on Windows (Prefix + Normal),
        // so the multi-component guard rejects it before the colon rule is
        // reached. The probe must be a single Normal component on BOTH
        // platforms for the message contract asserted here to hold.
        let err = require_safe_filename("filename", "stream.txt:ads").unwrap_err();
        assert_eq!(
            err.message,
            "no debe contener ':' (riesgo de flujos alternativos NTFS)"
        );
        assert_eq!(tag(&err).1.as_deref(), Some(REASON_PATH_NOT_ALLOWED));
    }

    #[test]
    fn reason_of_tolerates_a_missing_or_foreign_data_payload() {
        // `data` is sender-defined, so the success-shaped `validate_url`
        // channel may see anything: absent, the pre-EC-08 bare string, or an
        // object without a `reason`. None of those may panic.
        assert_eq!(reason_of(None), None);
        assert_eq!(reason_of(Some(&Value::String("url".to_string()))), None);
        assert_eq!(reason_of(Some(&json!({"field": "url"}))), None);
        assert_eq!(reason_of(Some(&json!({"reason": 42}))), None);
        assert_eq!(
            reason_of(Some(&json!({"reason": REASON_MALFORMED}))),
            Some(REASON_MALFORMED)
        );
    }
}
