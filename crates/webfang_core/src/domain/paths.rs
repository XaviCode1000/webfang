//! Platform user-directory resolution — the ONE definition of where webfang
//! puts its config and cache files (XP-F-05, #1608).
//!
//! Four call sites used to hand-roll `~/.config` / `~/.cache` with raw
//! `XDG_*` + `$HOME` logic, which baked the Linux convention into every
//! platform. All sites now go through these helpers, backed by the `dirs`
//! crate:
//!
//! - **Linux**: XDG semantics, unchanged — `dirs` honors `$XDG_CONFIG_HOME`
//!   / `$XDG_CACHE_HOME` and falls back to `$HOME/.config` / `$HOME/.cache`.
//! - **Windows**: `%APPDATA%` (Roaming) for config, `%LOCALAPPDATA%` for
//!   cache.
//! - **macOS**: `~/Library/Application Support` for config, `~/Library/Caches`
//!   for cache.
//!
//! On top of the platform convention, an ABSOLUTE `$XDG_CONFIG_HOME` /
//! `$XDG_CACHE_HOME` wins on every platform (dirs only honors them on
//! Linux). This is deliberate, not Linux leakage: the repo's hermeticity
//! contract (#1126) and several tests inject these vars to redirect webfang's
//! writes to a temp dir, and silently ignoring the injection on macOS/Windows
//! would make tests read and write the operator's real profile directories
//! (evidenced by `test_fresh_cache_served_from_disk_off_the_executor`
//! failing on both non-Linux CI lanes). The absolute-path requirement keeps
//! the same strictness `dirs` applies on Linux.
//!
//! Callers append the webfang-specific component (`webfang/…`) themselves so
//! the base-dir policy and the file-layout policy stay separable.

use std::path::PathBuf;

/// Base directory for user configuration files.
///
/// `None` means the platform could not produce a user home (e.g. no `$HOME`
/// and no `%APPDATA%`): callers pick their own fail-soft fallback, which is
/// the behavior each site already had.
#[must_use]
pub(crate) fn config_base_dir() -> Option<PathBuf> {
    xdg_override("XDG_CONFIG_HOME").or_else(dirs::config_dir)
}

/// Base directory for cache / state files.
///
/// `None` means the platform could not produce a user home: callers pick
/// their own fail-soft fallback, matching their previous behavior.
#[must_use]
pub(crate) fn cache_base_dir() -> Option<PathBuf> {
    xdg_override("XDG_CACHE_HOME").or_else(dirs::cache_dir)
}

/// Explicit XDG override, honored on every platform when ABSOLUTE (matching
/// the strictness `dirs` applies on Linux). Empty or relative values are
/// ignored.
fn xdg_override(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux the helpers must honor XDG env vars exactly like the
    /// hand-rolled code they replaced — this pins the byte-identical
    /// Linux contract of XP-F-05.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_config_base_dir_honors_xdg_config_home() {
        use webfang_test_utils::EnvGuard;

        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let guard = EnvGuard::with(&[("XDG_CONFIG_HOME", tmp.path().to_str().expect("utf8 tmp"))]);
        assert_eq!(config_base_dir(), Some(tmp.path().to_path_buf()));
        drop(guard);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_cache_base_dir_honors_xdg_cache_home() {
        use webfang_test_utils::EnvGuard;

        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let guard = EnvGuard::with(&[("XDG_CACHE_HOME", tmp.path().to_str().expect("utf8 tmp"))]);
        assert_eq!(cache_base_dir(), Some(tmp.path().to_path_buf()));
        drop(guard);
    }

    /// A missing XDG var must not produce a crash or an empty path: the
    /// fallback chain (home-relative) still yields something usable.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_base_dirs_fall_back_to_home_relative_paths() {
        use webfang_test_utils::EnvGuard;

        let guard = EnvGuard::with(&[("XDG_CONFIG_HOME", ""), ("XDG_CACHE_HOME", "")]);
        // In a sandbox without $HOME this is None; with $HOME it is
        // $HOME/.config — both are the documented, non-panicking outcomes.
        if std::env::var_os("HOME").is_some() {
            assert!(config_base_dir().is_some());
            assert!(cache_base_dir().is_some());
        }
        drop(guard);
    }

    /// The XDG override must be ABSOLUTE to be honored (the same strictness
    /// `dirs` applies on Linux): a relative value must fall through to the
    /// platform convention instead of being taken verbatim. This is the
    /// contract the hermeticity harness (#1126) and the non-Linux CI lanes
    /// rely on.
    #[test]
    fn relative_xdg_override_is_ignored() {
        use webfang_test_utils::EnvGuard;

        let guard = EnvGuard::with(&[("XDG_CONFIG_HOME", "relative/config")]);
        let base = config_base_dir();
        assert_ne!(
            base.as_deref(),
            Some(std::path::Path::new("relative/config")),
            "a relative XDG override must never be taken verbatim"
        );
        drop(guard);
    }
}
