//! Canonical path-confinement gate shared by every MCP write path (#1588).
//!
//! Every tool parameter that names a filesystem write destination — `output_dir`
//! (export roots, #696) and `crawl_site`'s `checkpoint_dir` — passes through
//! [`confine`], which enforces, in order:
//!
//! 1. non-empty and ≤ [`MAX_PATH_LEN`];
//! 2. rejection of rooted-but-not-absolute forms ([`PathShape::RootedNotAbsolute`]:
//!    `\foo`, `C:foo`, drive-relative paths). `Path::is_absolute` is
//!    platform-specific, so a form the host does not consider absolute must
//!    never be silently treated as "relative";
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

use super::validation::{invalid_params, MAX_PATH_LEN};

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
    loop {
        let Some(name) = cursor.file_name() else {
            break;
        };
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
/// component-based ([`Path::starts_with`]), never a string prefix, so sibling
/// directories (`/srv/exports_evil` vs `/srv/exports`) do not match.
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
        .any(|root| resolved.starts_with(resolve_fully(root)))
}

/// The shape of a raw path string, decided WITHOUT relying on
/// `Path::is_absolute` alone — that predicate is platform-specific and would
/// let `C:foo`/`\foo` through as "relative" on Unix hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathShape {
    /// Purely relative (`exports/2026`, `./output`) — resolves against CWD.
    Relative,
    /// Absolute on this host (`/srv/exports`, `C:\exports` on Windows).
    Absolute,
    /// Rooted but NOT absolute: `\foo` (leading backslash / UNC-ish),
    /// drive-relative `C:foo`/`C:`. Rejected on every platform — never a
    /// safe relative path.
    RootedNotAbsolute,
}

/// Detect a Windows-style drive-letter prefix (letter + `:`) regardless of
/// host platform and WITHOUT requiring a separator, so the drive-relative
/// `C:foo` form — a path escape on Windows hosts — is classified too.
fn has_drive_prefix(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Classify a raw path string into a [`PathShape`].
///
/// Probes the raw string in addition to [`Path::has_root`] because on Unix a
/// leading `\` is an ordinary filename byte yet must still be rejected (the
/// server may be fronted by or shared with Windows volumes, and no Unix tool
/// needs it).
pub(crate) fn classify(raw: &str) -> PathShape {
    let path = Path::new(raw);
    if path.is_absolute() {
        return PathShape::Absolute;
    }
    if raw.starts_with('\\') || has_drive_prefix(raw) || path.has_root() {
        return PathShape::RootedNotAbsolute;
    }
    PathShape::Relative
}

/// The single confinement check for every MCP filesystem write destination
/// (#1588). See the module docs for the enforced order.
///
/// # Errors
/// Returns `McpError::invalid_params` for empty/oversize input, rooted
/// non-absolute forms, `..` traversal, absolute paths with no configured
/// roots, and absolute paths outside every resolved root. Every rejection
/// carries a structured `tracing::warn!` (field, path, roots — message stays
/// static per the observability conventions).
pub(crate) fn confine(field: &str, raw: &str, roots: &[PathBuf]) -> Result<(), McpError> {
    if raw.is_empty() {
        return Err(invalid_params(field, "must not be empty"));
    }
    if raw.len() > MAX_PATH_LEN {
        return Err(invalid_params(
            field,
            format!("exceeds maximum length of {MAX_PATH_LEN} bytes"),
        ));
    }
    let shape = classify(raw);
    if matches!(shape, PathShape::RootedNotAbsolute) {
        tracing::warn!(field, path = raw, "path rejected: rooted non-absolute form");
        return Err(invalid_params(
            field,
            "must be absolute or relative; rooted non-absolute forms ('\\foo', 'C:foo') \
             are not allowed",
        ));
    }
    let path = Path::new(raw);
    // Reject `..` BEFORE any lexical normalization: normalize_lexical resolves
    // `..` before symlinks, which is the wrong order for containment.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(invalid_params(
            field,
            "must not contain '..' traversal components",
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
        return Err(invalid_params(
            field,
            format!(
                "absolute {field} requires server-configured export roots (none configured); \
                 set --export-roots or use a relative path"
            ),
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
    Err(invalid_params(
        field,
        format!("{field} '{raw}' is outside allowed export roots"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        #[cfg(unix)]
        {
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
        assert_eq!(classify("C:\\Windows"), PathShape::Absolute);
        assert_eq!(classify("/foo"), PathShape::RootedNotAbsolute);
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
        for raw in ["C:foo", "\\foo", "c:/x"] {
            let err = confine("output_dir", raw, &[])
                .expect_err("rooted non-absolute form must be rejected");
            assert!(
                err.to_string().contains("rooted non-absolute"),
                "{raw}: got: {err}"
            );
        }
    }

    #[test]
    fn confine_rejects_dotdot_before_containment() {
        // Absolute `..`: rejected up-front (lexical `..` resolution would run
        // before symlinks — wrong order for containment).
        let roots = vec![PathBuf::from("/srv/exports")];
        let err = confine("output_dir", "/srv/exports/../other", &roots)
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
        let err = confine("output_dir", "/srv/exports/x", &[])
            .expect_err("absolute path with no roots must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("requires server-configured export roots"),
            "error must name the missing export roots, got: {msg}"
        );
        // The field name is interpolated so `checkpoint_dir` errors are
        // distinguishable from `output_dir` ones.
        let err = confine("checkpoint_dir", "/srv/checkpoints", &[])
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
        let roots = vec![PathBuf::from("/srv/exports")];
        confine("output_dir", "/srv/exports/sub/file.txt", &roots)
            .expect("path under a configured root must be allowed");
        // The root itself is a valid destination.
        confine("output_dir", "/srv/exports", &roots)
            .expect("the root path itself must be allowed");
    }

    #[test]
    fn confine_absolute_outside_root_rejected() {
        let roots = vec![PathBuf::from("/srv/exports")];
        let err = confine("output_dir", "/etc/passwd", &roots)
            .expect_err("path outside every root must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("outside allowed export roots"),
            "error must name the root violation, got: {msg}"
        );
    }

    #[test]
    fn confine_sibling_prefix_is_not_a_match() {
        // `/srv/exports_evil` must NOT satisfy root `/srv/exports` — the check
        // is component-based (`starts_with`), not string-based.
        let roots = vec![PathBuf::from("/srv/exports")];
        confine("output_dir", "/srv/exports_evil/file", &roots)
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
