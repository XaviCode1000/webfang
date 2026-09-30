//! Canonical path-confinement gate shared by every MCP write path (#1588).
//!
//! Every tool parameter that names a filesystem write destination — `output_dir`
//! (export roots, #696) and `crawl_site`'s `checkpoint_dir` — passes through
//! [`confine`], which enforces, in order:
//!
//! 1. non-empty and ≤ [`MAX_PATH_LEN`];
//! 2. classification into a [`PathShape`] (platform-aware, see the table
//!    below) and rejection of [`PathShape::RootedNotAbsolute`] forms.
//!    `Path::is_absolute` is platform-specific, so a form the host does not
//!    consider absolute must never be silently treated as "relative";
//! 3. rejection of any `..` component (lexical normalization would resolve it
//!    *before* symlinks — exactly what this gate must not do);
//! 4. relative paths pass (they resolve against the server's CWD, the #696
//!    contract — the server's own boundary does not apply to them);
//! 5. absolute paths must be contained in the configured roots — fail-closed
//!    when no roots are configured. Containment is checked AFTER [`resolve_fully`]
//!    resolves symlinks/junctions on BOTH sides, so a symlink inside a root
//!    that points outside it is rejected (the purely lexical check this
//!    replaces could not see it).
//!
//! This order is the SECURITY order (fail-closed preconditions before any
//! work) and is IDENTICAL on every platform (#1608): only the classification
//! inputs differ per host — the precedence of rooted-form vs missing-roots vs
//! `..` vs containment never does.
//!
//! ## Classification semantics (path form × host platform)
//!
//! | Form | Windows host | POSIX host |
//! | :--- | :--- | :--- |
//! | `exports/2026`, `./output` | Relative | Relative |
//! | `/srv/exports` | `RootedNotAbsolute` — root WITHOUT a drive prefix is current-drive-relative under std semantics | `Absolute` → containment |
//! | `\foo` | `RootedNotAbsolute` — same rule (root without prefix) | `RootedNotAbsolute` (lexical probe; `\` is an ordinary filename byte on Unix) |
//! | `C:\x`, `c:/x` | `Absolute` (prefix + root) → containment | `RootedNotAbsolute` (not absolute on POSIX) |
//! | `C:foo`, `C:` | `RootedNotAbsolute` — drive-RELATIVE (prefix, no root separator; XP-P-01) | `RootedNotAbsolute` |
//! | `\\server\share` (UNC) | `Absolute` → containment | `RootedNotAbsolute` |
//!
//! Fail-closed rationale (#1608): drive-absolute forms (`C:\exports`) are
//! ordinary absolute paths on a Windows host and flow through the same
//! containment pipeline (canonicalize + within-roots + symlink resolution)
//! as POSIX absolutes — no shortcut. Drive-relative (`C:foo`) and
//! root-without-prefix (`\foo`, `/foo`) forms depend on an OS-level implicit
//! current drive; `std` does not consider them absolute on Windows either, so
//! the gate rejects them there too rather than guessing a drive.
//!
//! ## Residual TOCTOU window (explicitly out of scope for #1588)
//!
//! This gate canonicalizes at VALIDATION time. An actor who can write inside a
//! declared root may still swap a validated directory for a symlink between
//! this check and the actual `create_dir_all`/write, escaping the root. Closing
//! that window needs directory-handle-relative writes (`openat`/`O_NOFOLLOW`)
//! at every writer — a much larger change deliberately NOT part of #1588,
//! whose requested fix is validation-time canonicalization of the purely
//! lexical check.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use rmcp::ErrorData as McpError;

use super::validation::{
    invalid_params_with_reason, MAX_PATH_LEN, REASON_EMPTY, REASON_PATH_NOT_ALLOWED,
    REASON_TOO_LONG,
};

/// Normalize a path lexically: resolve `.` and `..` components without
/// filesystem access. Applied first by [`resolve_fully`] so redundant
/// components cannot defeat the prefix check (#696), and by the #1588 gate
/// only AFTER it has rejected any raw `..` component (lexical `..` resolution
/// happens before symlinks — the wrong order for containment).
fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {},
            Component::ParentDir => {
                out.pop();
            },
            other => out.push(other),
        }
    }
    out
}

/// Fully resolve `path`: lexically normalize, then canonicalize.
///
/// `std::fs::canonicalize` requires the whole path to exist, but write targets
/// usually do not exist yet. So on failure this walks UP the lexical path,
/// popping `file_name()` components onto a tail until the longest existing
/// prefix canonicalizes (resolving every symlink along it), then re-appends the
/// tail in reverse. When nothing canonicalizes (e.g. a bare relative path with
/// no existing ancestor), the lexically normalized path is returned — never a
/// panic, never an error: a non-existent leaf cannot itself be a symlink, and
/// any existing ancestor was already resolved by the walk.
///
/// The walk terminates because each iteration moves one component closer to
/// the root, and `file_name()` is `None` for the root/drive itself.
fn resolve_fully(path: &Path) -> PathBuf {
    let lexical = normalize_lexical(path);
    if let Ok(resolved) = std::fs::canonicalize(&lexical) {
        return resolved;
    }
    let mut tail: Vec<OsString> = Vec::new();
    let mut cursor = lexical.as_path();
    while let Some(name) = cursor.file_name() {
        tail.push(name.to_os_string());
        let Some(parent) = cursor.parent() else {
            break;
        };
        cursor = parent;
        if let Ok(resolved) = std::fs::canonicalize(cursor) {
            let mut out = resolved;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
    }
    lexical
}

/// True when `path` is contained in one of `roots` after FULL resolution of
/// both sides (#1588).
///
/// Symlinks/junctions are resolved via [`resolve_fully`] before the check, so
/// a link inside a root that points outside it does not match. The match is
/// component-based (see [`components_start_with`]), never a string prefix, so
/// sibling directories (`/srv/exports_evil` vs `/srv/exports`) do not match.
/// On case-insensitive filesystems (NTFS, APFS) the component comparison is
/// case-insensitive too (XP-P-02, issue #1608).
///
/// Empty `roots` always yields `false` — the CALLER owns the empty-roots
/// policy: [`confine`] rejects absolute paths (fail-closed) and
/// `McpState::with_export_roots` skips its #769 startup consistency check.
pub(crate) fn resolved_within_roots(path: &Path, roots: &[PathBuf]) -> bool {
    if roots.is_empty() {
        return false;
    }
    let resolved = resolve_fully(path);
    roots
        .iter()
        .any(|root| components_start_with(&resolved, &resolve_fully(root), FS_CASE_INSENSITIVE))
}

/// Compile-time case-sensitivity of the target filesystem family (XP-P-02,
/// issue #1608): Windows (NTFS) and macOS (APFS default) compare paths
/// case-insensitively, so a root declared as `/Users/Xavi/exports` must
/// contain a candidate spelled `/users/xavi/exports`. `cfg!` is evaluated at
/// compile time — on Linux this constant is `false` and the containment
/// behavior is byte-identical to the previous `Path::starts_with` check.
const FS_CASE_INSENSITIVE: bool = cfg!(windows) || cfg!(target_os = "macos");

/// Component-based prefix containment with an explicit case-sensitivity
/// switch.
///
/// `case_insensitive = false` behaves byte-identically to
/// [`Path::starts_with`]. With `true`, each overlapping component compares
/// ASCII-lowercased lossy strings, so a root stated with different case still
/// contains the candidate on case-insensitive filesystems (NTFS, APFS). The
/// check stays component-based in both branches: a string-prefix sibling
/// (`/srv/exports_evil`) never matches.
fn components_start_with(path: &Path, root: &Path, case_insensitive: bool) -> bool {
    let mut path_comps = path.components();
    for root_comp in root.components() {
        match path_comps.next() {
            Some(path_comp) => {
                let equal = if case_insensitive {
                    path_comp.as_os_str().to_string_lossy().to_lowercase()
                        == root_comp.as_os_str().to_string_lossy().to_lowercase()
                } else {
                    path_comp.as_os_str() == root_comp.as_os_str()
                };
                if !equal {
                    return false;
                }
            },
            // The path ended before the root was consumed: not contained.
            None => return false,
        }
    }
    true
}

/// The shape of a raw path string, decided WITHOUT relying on
/// `Path::is_absolute` alone — that predicate is platform-specific and would
/// let `C:foo`/`\foo` through as "relative" on Unix hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathShape {
    /// Purely relative (`exports/2026`, `./output`) — resolves against CWD.
    Relative,
    /// Absolute on this host: `/srv/exports` on POSIX; `C:\exports`, `c:/x`
    /// and UNC `\\server\share` on Windows. Flows through the containment
    /// pipeline (roots → canonicalize → within-roots).
    Absolute,
    /// Rooted but NOT absolute: drive-relative `C:foo`/`C:` (every
    /// platform), `\foo`/`/foo` on Windows (root without drive prefix —
    /// current-drive-relative under std semantics), and `C:\x`/`\foo` on
    /// POSIX (the host does not consider them absolute). Rejected on every
    /// platform — never a safe relative path.
    RootedNotAbsolute,
}

/// Detect a Windows-style drive-letter prefix (letter + `:`) regardless of
/// host platform and WITHOUT requiring a separator, so the drive-relative
/// `C:foo` form — a path escape on Windows hosts — is classified too.
fn has_drive_prefix(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Classify a raw path string into a [`PathShape`] for the compiling host.
///
/// The platform-specific answers come from `std` (authoritative: the gate
/// must always agree with the `canonicalize`/containment layer below it) and
/// are passed as data to [`classify_with`], the pure core. The raw string is
/// probed IN ADDITION to [`Path::has_root`] because on Unix a leading `\` is
/// an ordinary filename byte yet must still be rejected (the server may be
/// fronted by or shared with Windows volumes, and no Unix tool needs it).
pub(crate) fn classify(raw: &str) -> PathShape {
    let path = Path::new(raw);
    classify_with(raw, path.is_absolute(), path.has_root())
}

/// Pure, host-independent core of [`classify`] (#1608).
///
/// `is_absolute`/`has_root` are the platform-specific `std::path` answers for
/// `raw`. Production passes the compiling host's answers; tests pass
/// simulated answers for either platform, which makes BOTH branches of the
/// semantics table in the module docs unit-testable on a Linux host.
///
/// Ordering inside the core is fail-closed-first: anything std does not vouch
/// for as absolute, but that is rooted or drive-prefixed, lands in
/// [`PathShape::RootedNotAbsolute`] — never silently in `Relative`. The
/// drive-prefix probe is what rejects drive-relative forms (`C:foo`, `C:`):
/// std gives them NO root on any platform, so `has_root` alone would miss
/// them (XP-P-01).
fn classify_with(raw: &str, is_absolute: bool, has_root: bool) -> PathShape {
    if is_absolute {
        return PathShape::Absolute;
    }
    if has_root || raw.starts_with('\\') || has_drive_prefix(raw) {
        return PathShape::RootedNotAbsolute;
    }
    PathShape::Relative
}

/// Map a POSIX-spelled absolute test literal to a host-appropriate absolute
/// path (test-only, shared by the `path_gate`, `state` and export-handler
/// tests, #1608).
///
/// On a Windows host `/srv/exports` is a root-without-prefix form
/// (current-drive-relative under std semantics) — NOT the absolute path the
/// absolute-containment tests mean — so there it is spelled `C:\srv\exports`
/// to exercise the real absolute pipeline. On POSIX hosts this is the
/// identity, keeping local Linux CI byte-identical to the previous tests.
#[cfg(test)]
pub(crate) fn host_abs(p: &str) -> String {
    if cfg!(windows) {
        format!("C:\\{}", p.trim_start_matches('/')).replace('/', "\\")
    } else {
        p.to_string()
    }
}

/// The single confinement check for every MCP filesystem write destination
/// (#1588). See the module docs for the enforced order.
///
/// # Errors
/// Returns `McpError::invalid_params` for empty/oversize input, rooted
/// non-absolute forms, `..` traversal, absolute paths with no configured
/// roots, and absolute paths outside every resolved root. Every rejection
/// carries a stable slug (EC-08, #1613): empty and oversize report
/// [`REASON_EMPTY`] / [`REASON_TOO_LONG`] because their remedy is to fill in
/// or shorten the value, while the four remaining branches share
/// [`REASON_PATH_NOT_ALLOWED`] — the caller's remedy ("use a safe path inside
/// an allowed export root") is the same for all of them and the message
/// carries the specific rule. Every rejection also emits a structured
/// `tracing::warn!` (field, path, roots; message stays static per the
/// observability conventions).
pub(crate) fn confine(field: &str, raw: &str, roots: &[PathBuf]) -> Result<(), McpError> {
    if raw.is_empty() {
        return Err(invalid_params_with_reason(
            field,
            "must not be empty",
            REASON_EMPTY,
        ));
    }
    if raw.len() > MAX_PATH_LEN {
        return Err(invalid_params_with_reason(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
            REASON_TOO_LONG,
        ));
    }
    let shape = classify(raw);
    if matches!(shape, PathShape::RootedNotAbsolute) {
        tracing::warn!(field, path = raw, "path rejected: rooted non-absolute form");
        return Err(invalid_params_with_reason(
            field,
            "must be absolute or relative; rooted non-absolute forms ('\\foo', 'C:foo') \
             are not allowed",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    let path = Path::new(raw);
    // Reject `..` BEFORE any lexical normalization: normalize_lexical resolves
    // `..` before symlinks, which is the wrong order for containment.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params_with_reason(
            field,
            "must not contain '..' traversal components",
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if matches!(shape, PathShape::Relative) {
        // Relative paths resolve against the server's CWD (#696 contract);
        // the server's own boundary does not apply to them.
        return Ok(());
    }
    // Absolute from here on.
    if roots.is_empty() {
        tracing::warn!(
            field,
            path = raw,
            "absolute path rejected: no export roots configured"
        );
        return Err(invalid_params_with_reason(
            field,
            format!(
                "absolute {field} requires server-configured export roots (none configured); \
                 set --export-roots or use a relative path"
            ),
            REASON_PATH_NOT_ALLOWED,
        ));
    }
    if resolved_within_roots(path, roots) {
        return Ok(());
    }
    tracing::warn!(
        field,
        path = raw,
        roots = ?roots,
        "path outside allowed export roots"
    );
    Err(invalid_params_with_reason(
        field,
        format!("{field} '{raw}' is outside allowed export roots"),
        REASON_PATH_NOT_ALLOWED,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    // --- classify ---------------------------------------------------------

    #[test]
    fn classify_relative_forms() {
        assert_eq!(classify("exports/2026"), PathShape::Relative);
        assert_eq!(classify("./output"), PathShape::Relative);
        assert_eq!(classify("output"), PathShape::Relative);
    }

    #[test]
    fn classify_rooted_not_absolute_forms() {
        // Drive-relative forms — the #1588 escape class — rejected on EVERY
        // platform, with or without a separator.
        assert_eq!(classify("C:foo"), PathShape::RootedNotAbsolute);
        assert_eq!(classify("c:foo"), PathShape::RootedNotAbsolute);
        assert_eq!(classify("C:"), PathShape::RootedNotAbsolute);
        // Leading backslash / UNC-ish form.
        assert_eq!(classify("\\foo"), PathShape::RootedNotAbsolute);
        #[cfg(not(windows))]
        {
            // Drive-absolute spellings are NOT absolute on a POSIX host —
            // they stay fail-closed there (byte-identical to the pre-#1608
            // behavior on Linux).
            assert_eq!(classify("c:/x"), PathShape::RootedNotAbsolute);
            assert_eq!(classify("C:\\Windows"), PathShape::RootedNotAbsolute);
            assert_eq!(classify("\\\\server\\share"), PathShape::RootedNotAbsolute);
        }
    }

    #[cfg(unix)]
    #[test]
    fn classify_absolute_unix() {
        assert_eq!(classify("/srv/exports"), PathShape::Absolute);
        assert_eq!(classify("/"), PathShape::Absolute);
    }

    #[cfg(windows)]
    #[test]
    fn classify_absolute_windows() {
        // Drive-absolute and UNC forms are ordinary absolute paths on a
        // Windows host (prefix + root) — #1608.
        assert_eq!(classify("C:\\Windows"), PathShape::Absolute);
        assert_eq!(classify("c:/x"), PathShape::Absolute);
        assert_eq!(classify("\\\\server\\share"), PathShape::Absolute);
        // Root WITHOUT a prefix stays rejected (current-drive-relative under
        // std semantics; fail-closed).
        assert_eq!(classify("/foo"), PathShape::RootedNotAbsolute);
        assert_eq!(classify("\\foo"), PathShape::RootedNotAbsolute);
    }

    /// Simulated `std::path` answers on a WINDOWS host, lexically derived and
    /// host-independent: `is_absolute` = UNC or a drive prefix followed by a
    /// root separator; `has_root` = leading separator. Only the well-known
    /// forms the tests exercise are covered — production always consults the
    /// real std answers for the compiling host.
    fn windows_host_answers(raw: &str) -> (bool, bool) {
        let bytes = raw.as_bytes();
        let unc = raw.starts_with("\\\\");
        let drive_absolute =
            has_drive_prefix(raw) && bytes.len() >= 3 && (bytes[2] == b'\\' || bytes[2] == b'/');
        let has_root = raw.starts_with('\\') || raw.starts_with('/');
        (unc || drive_absolute, has_root)
    }

    /// Both branches of the classification table must hold REGARDLESS of the
    /// compiling host, so they are pinned through the pure core with
    /// simulated std answers (deterministic on Linux CI, #1608).
    #[test]
    fn classify_with_reproduces_windows_host_semantics() {
        let cases: &[(&str, PathShape)] = &[
            ("C:\\exports", PathShape::Absolute),
            ("c:/x", PathShape::Absolute),
            ("\\\\server\\share", PathShape::Absolute),
            // XP-P-01: drive-RELATIVE — prefix without a root separator.
            ("C:foo", PathShape::RootedNotAbsolute),
            ("C:", PathShape::RootedNotAbsolute),
            // Root without prefix (current-drive-relative under std).
            ("\\foo", PathShape::RootedNotAbsolute),
            ("/foo", PathShape::RootedNotAbsolute),
            ("foo", PathShape::Relative),
            ("foo\\bar", PathShape::Relative),
        ];
        for (raw, expected) in cases {
            let (is_absolute, has_root) = windows_host_answers(raw);
            assert_eq!(
                classify_with(raw, is_absolute, has_root),
                *expected,
                "wrong Windows-host shape for {raw}"
            );
        }
    }

    #[test]
    fn classify_with_reproduces_posix_host_semantics() {
        // POSIX std answers, lexically simulated: absolute iff leading `/`
        // (and `has_root` then says the same thing).
        let cases: &[(&str, PathShape)] = &[
            ("/srv/exports", PathShape::Absolute),
            // Byte-identical to the pre-#1608 behavior on POSIX hosts:
            ("C:\\Windows", PathShape::RootedNotAbsolute),
            ("c:/x", PathShape::RootedNotAbsolute),
            ("\\\\server\\share", PathShape::RootedNotAbsolute),
            ("\\foo", PathShape::RootedNotAbsolute),
            ("C:foo", PathShape::RootedNotAbsolute),
            ("foo", PathShape::Relative),
        ];
        for (raw, expected) in cases {
            let absolute = raw.starts_with('/');
            assert_eq!(
                classify_with(raw, absolute, absolute),
                *expected,
                "wrong POSIX-host shape for {raw}"
            );
        }
    }

    /// XP-P-01 re-check (#1608): the gate must never classify by
    /// `is_absolute()` alone. Drive-relative forms have NO std root on any
    /// platform (`C:foo` on Windows: prefix without root separator), so the
    /// lexical drive-prefix probe is what rejects them. Pinned here under
    /// simulated WINDOWS host answers — runnable on Linux.
    #[test]
    fn drive_relative_forms_rejected_under_windows_semantics() {
        for raw in ["C:foo", "c:foo", "C:"] {
            let (is_absolute, has_root) = windows_host_answers(raw);
            assert_eq!(
                classify_with(raw, is_absolute, has_root),
                PathShape::RootedNotAbsolute,
                "drive-relative '{raw}' must stay rejected on a Windows host"
            );
        }
    }

    // --- confine: the EC-08 reason slug (issue #1613) -----------------------

    /// EC-08: every rejection this gate emits must carry BOTH halves of
    /// `error.data` — the offending `field` and the stable `reason` slug — so
    /// a caller never has to parse the message to know which rule fired.
    ///
    /// The JSON-RPC code (`INVALID_PARAMS`) and the message text are pinned by
    /// the tests above; this one covers the machine-readable payload, and it
    /// deliberately samples EVERY rejection branch of `confine` (they all
    /// share the one slug).
    #[test]
    fn every_rejection_carries_field_and_reason_slug() {
        let roots = vec![PathBuf::from(host_abs("/srv/exports"))];
        let oversize = "a".repeat(MAX_PATH_LEN + 1);
        let no_roots = host_abs("/srv/exports/x");
        let outside = host_abs("/srv/checkpoints");
        let cases: [(&str, &str, &[PathBuf], &str); 6] = [
            // empty — its own remedy ("fill it in") beats the path slug
            ("output_dir", "", &roots, REASON_EMPTY),
            // oversize — likewise
            ("output_dir", &oversize, &roots, REASON_TOO_LONG),
            // rooted non-absolute
            ("output_dir", "\\foo", &roots, REASON_PATH_NOT_ALLOWED),
            // `..` traversal
            (
                "output_dir",
                "sub/../other",
                &roots,
                REASON_PATH_NOT_ALLOWED,
            ),
            // absolute with no roots configured (fail-closed)
            ("output_dir", &no_roots, &[], REASON_PATH_NOT_ALLOWED),
            // absolute outside every root
            ("checkpoint_dir", &outside, &roots, REASON_PATH_NOT_ALLOWED),
        ];
        for (field, raw, roots, expected_reason) in cases {
            let err = confine(field, raw, roots).expect_err("must be rejected");
            assert_eq!(
                err.code,
                rmcp::model::ErrorCode::INVALID_PARAMS,
                "{raw}: the JSON-RPC code must stay -32602, got: {err:?}"
            );
            let data = err.data.as_ref().expect("every rejection carries `data`");
            assert!(
                data.is_object(),
                "`data` must be a JSON OBJECT (not a bare string): {data}"
            );
            assert_eq!(
                data.get("field").and_then(Value::as_str),
                Some(field),
                "{raw}: `data.field` must name the offending field: {data}"
            );
            assert_eq!(
                data.get("reason").and_then(Value::as_str),
                Some(expected_reason),
                "{raw}: `data.reason` must carry the stable slug: {data}"
            );
        }
    }

    // --- confine: shape-level rejections ----------------------------------

    #[test]
    fn confine_relative_passes_with_no_roots() {
        confine("output_dir", "exports/2026", &[])
            .expect("relative paths must pass through with no roots configured");
        confine("checkpoint_dir", "./checkpoints", &[])
            .expect("relative checkpoint dirs must pass with no roots configured");
    }

    #[test]
    fn confine_rejects_empty_and_oversize() {
        let err = confine("output_dir", "", &[]).expect_err("empty must be rejected");
        assert!(err.to_string().contains("must not be empty"), "got: {err}");
        let oversize = "a".repeat(MAX_PATH_LEN + 1);
        let err = confine("output_dir", &oversize, &[]).expect_err("oversize must be rejected");
        assert!(
            err.to_string()
                .contains(&format!("exceeds maximum length of {MAX_PATH_LEN} bytes")),
            "got: {err}"
        );
    }

    #[test]
    fn confine_rejects_rooted_not_absolute_forms() {
        // Drive-relative forms — the #1588 escape class — rejected on EVERY
        // platform, with or without a separator; leading backslash too.
        for raw in ["C:foo", "c:foo", "C:", "\\foo"] {
            let err = confine("output_dir", raw, &[])
                .expect_err("rooted non-absolute form must be rejected");
            assert!(
                err.to_string().contains("rooted non-absolute"),
                "{raw}: got: {err}"
            );
        }
        // `c:/x` is drive-ABSOLUTE on a Windows host (prefix + root): there
        // it flows into the normal absolute pipeline (no roots →
        // missing-roots error), while on POSIX hosts it stays a rooted
        // non-absolute form. The precedence ORDER is identical on every
        // platform — only the classification input differs (#1608).
        #[cfg(not(windows))]
        {
            let err = confine("output_dir", "c:/x", &[])
                .expect_err("drive-absolute spelling must be a rooted form on POSIX hosts");
            assert!(
                err.to_string().contains("rooted non-absolute"),
                "got: {err}"
            );
        }
        #[cfg(windows)]
        {
            let err = confine("output_dir", "c:/x", &[])
                .expect_err("drive-absolute spelling is a real absolute path on Windows");
            assert!(
                err.to_string()
                    .contains("requires server-configured export roots"),
                "got: {err}"
            );
        }
    }

    #[test]
    fn confine_rejects_dotdot_before_containment() {
        // Absolute `..`: rejected up-front (lexical `..` resolution would run
        // before symlinks — wrong order for containment). The root and the
        // candidate use host-appropriate absolute spellings (`host_abs`), so
        // the `..` check — not a rooted-form rejection — is what fires on
        // every platform.
        let roots = vec![PathBuf::from(host_abs("/srv/exports"))];
        let err = confine("output_dir", &host_abs("/srv/exports/../other"), &roots)
            .expect_err("`..` must be rejected before any containment logic");
        assert!(err.to_string().contains("'..' traversal"), "got: {err}");
        // Relative `..`: also rejected (behavior change vs. the old
        // `!is_absolute() → Ok` fast path, #1588).
        let err = confine("checkpoint_dir", "sub/../other", &[])
            .expect_err("`..` must be rejected even in relative form");
        assert!(err.to_string().contains("'..' traversal"), "got: {err}");
    }

    // --- confine: absolute containment -------------------------------------

    #[test]
    fn confine_absolute_without_roots_fails_closed() {
        let err = confine("output_dir", &host_abs("/srv/exports/x"), &[])
            .expect_err("absolute path with no roots must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("requires server-configured export roots"),
            "error must name the missing export roots, got: {msg}"
        );
        // The field name is interpolated so `checkpoint_dir` errors are
        // distinguishable from `output_dir` ones.
        let err = confine("checkpoint_dir", &host_abs("/srv/checkpoints"), &[])
            .expect_err("absolute checkpoint_dir with no roots must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("checkpoint_dir"), "got: {msg}");
        assert!(
            msg.contains("requires server-configured export roots"),
            "got: {msg}"
        );
    }

    #[test]
    fn confine_absolute_inside_root_allowed() {
        let roots = vec![PathBuf::from(host_abs("/srv/exports"))];
        confine("output_dir", &host_abs("/srv/exports/sub/file.txt"), &roots)
            .expect("path under a configured root must be allowed");
        // The root itself is a valid destination.
        confine("output_dir", &host_abs("/srv/exports"), &roots)
            .expect("the root path itself must be allowed");
    }

    #[test]
    fn confine_absolute_outside_root_rejected() {
        let roots = vec![PathBuf::from(host_abs("/srv/exports"))];
        let err = confine("output_dir", &host_abs("/etc/passwd"), &roots)
            .expect_err("path outside every root must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("outside allowed export roots"),
            "error must name the root violation, got: {msg}"
        );
    }

    #[test]
    fn confine_sibling_prefix_is_not_a_match() {
        // `<root>_evil` must NOT satisfy the root — the check is
        // component-based (`starts_with`), not string-based.
        let roots = vec![PathBuf::from(host_abs("/srv/exports"))];
        confine("output_dir", &host_abs("/srv/exports_evil/file"), &roots)
            .expect_err("string-prefix sibling dir must not match the root");
    }

    // --- resolved_within_roots (moved from state.rs, #769/#696 helpers) ----

    /// Empty `roots` is handled BY THE CALLER (fail-closed rejection in
    /// `confine`; the #769 startup check skips it) — the helper itself must
    /// never match.
    #[test]
    fn within_roots_empty_roots_never_matches() {
        assert!(
            !resolved_within_roots(Path::new("/srv/exports/file.txt"), &[]),
            "empty roots must yield false — the caller owns the empty-roots policy"
        );
    }

    #[test]
    fn within_roots_absolute_outside_returns_false() {
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            !resolved_within_roots(Path::new("/etc/passwd"), &roots),
            "a path outside every root must yield false"
        );
    }

    #[test]
    fn within_roots_absolute_inside_root_returns_true() {
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            resolved_within_roots(Path::new("/srv/exports/sub/file.txt"), &roots),
            "a path under a root must yield true"
        );
        // The root itself is a valid destination.
        assert!(
            resolved_within_roots(Path::new("/srv/exports"), &roots),
            "the root path itself must yield true"
        );
    }

    #[test]
    fn within_roots_dotdot_cannot_defeat_the_prefix_check() {
        let roots = vec![PathBuf::from("/srv/exports")];
        // The raw string starts with the root, but lexical normalization
        // resolves `..` first, landing at `/srv/other`.
        assert!(
            !resolved_within_roots(Path::new("/srv/exports/../other"), &roots),
            "`..` traversal out of the root must yield false"
        );
        // A candidate that traverses `..` and lands back INSIDE the root
        // stays allowed.
        assert!(
            resolved_within_roots(Path::new("/srv/exports/sub/../../exports/file"), &roots),
            "`..` segments that resolve back inside the root must yield true"
        );
    }

    #[test]
    fn within_roots_dot_components_are_ignored() {
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            resolved_within_roots(Path::new("/srv/./exports/./file.txt"), &roots),
            "redundant `.` components must not defeat a valid match"
        );
    }

    #[test]
    fn within_roots_sibling_prefix_is_not_a_root_match() {
        // `/srv/exports_evil` must NOT satisfy root `/srv/exports` — the
        // prefix check is component-based (`starts_with`), not string-based.
        let roots = vec![PathBuf::from("/srv/exports")];
        assert!(
            !resolved_within_roots(Path::new("/srv/exports_evil/file"), &roots),
            "a string-prefix sibling dir must yield false"
        );
    }

    // --- case sensitivity of the containment check (XP-P-02, #1608) ---------

    /// cfg! alone cannot be tested on Linux, so the comparison is factored
    /// into `components_start_with(case_insensitive)` and BOTH branches are
    /// exercised explicitly here.
    #[test]
    fn components_start_with_covers_both_sensitivity_branches() {
        let root = Path::new("/srv/Exports");

        // Case-insensitive branch (what NTFS/APFS hosts compile to): a root
        // declared with different case still contains the candidate.
        assert!(
            components_start_with(Path::new("/srv/exports/sub/file.txt"), root, true),
            "case-insensitive containment must match case-differing components"
        );
        assert!(
            components_start_with(Path::new("/SRV/EXPORTS"), root, true),
            "case-insensitive containment covers every component"
        );

        // Case-sensitive branch (what Linux compiles to): byte-identical to
        // the previous `Path::starts_with` behavior.
        assert!(
            !components_start_with(Path::new("/srv/exports/sub/file.txt"), root, false),
            "case-sensitive containment must reject case-differing components"
        );
        assert!(
            components_start_with(Path::new("/srv/Exports/sub/file.txt"), root, false),
            "case-sensitive containment still accepts the exact-case spelling"
        );

        // A string-prefix sibling matches in NEITHER branch.
        assert!(!components_start_with(
            Path::new("/srv/Exports_evil/f"),
            root,
            true
        ));
        assert!(!components_start_with(
            Path::new("/srv/Exports_evil/f"),
            root,
            false
        ));

        // The candidate ending before the root is not contained in either.
        assert!(!components_start_with(Path::new("/srv"), root, true));
        assert!(!components_start_with(Path::new("/srv"), root, false));
    }

    #[test]
    fn resolved_within_roots_stays_case_sensitive_on_this_host() {
        // Compile-time wiring check for THIS host: on Linux/macOS CI and
        // Windows runners the expectation differs, and `cfg!` inside the
        // test keeps both worlds honest.
        let roots = vec![PathBuf::from("/srv/Exports")];
        let case_differing = resolved_within_roots(Path::new("/srv/exports/f"), &roots);
        if FS_CASE_INSENSITIVE {
            assert!(case_differing, "case-insensitive host must match");
        } else {
            assert!(!case_differing, "case-sensitive host must not match");
        }
    }

    // --- symlink resolution (the #1588 fix itself) -------------------------

    #[cfg(unix)]
    #[test]
    fn symlink_escape_outside_root_is_rejected() {
        let root = tempfile::TempDir::new().expect("root temp dir");
        let outside = tempfile::TempDir::new().expect("outside temp dir");
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).expect("create symlink");
        let candidate = link.join("exports");
        let roots = vec![root.path().to_path_buf()];

        assert!(
            !resolved_within_roots(&candidate, &roots),
            "symlink pointing outside the root must not be contained"
        );
        let err = confine("output_dir", &candidate.to_string_lossy(), &roots)
            .expect_err("symlink escape must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("outside allowed export roots"), "got: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_inside_root_is_accepted() {
        let root = tempfile::TempDir::new().expect("root temp dir");
        let real = root.path().join("real");
        std::fs::create_dir(&real).expect("create real dir");
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("create symlink");
        let roots = vec![root.path().to_path_buf()];

        confine(
            "output_dir",
            &link.join("exports").to_string_lossy(),
            &roots,
        )
        .expect("symlink resolving inside the root must be accepted");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_root_matches_equivalent_spelling() {
        let root = tempfile::TempDir::new().expect("root temp dir");
        let alias_dir = tempfile::TempDir::new().expect("alias temp dir");
        let alias = alias_dir.path().join("alias");
        std::os::unix::fs::symlink(root.path(), &alias).expect("create root alias");

        // Candidate spelled through the alias, root stated canonically.
        let roots = vec![root.path().to_path_buf()];
        confine(
            "output_dir",
            &alias.join("exports").to_string_lossy(),
            &roots,
        )
        .expect("candidate through a root alias must be accepted");

        // Reverse: candidate canonical, root stated through the alias.
        let roots_via_alias = vec![alias];
        confine(
            "output_dir",
            &root.path().join("exports").to_string_lossy(),
            &roots_via_alias,
        )
        .expect("root stated through an alias must accept canonical candidates");
    }
}
