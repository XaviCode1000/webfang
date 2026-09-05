#![deny(missing_docs)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
//! Shared test utilities for the webfang workspace.
//!
//! Provides RAII environment isolation, output redaction for deterministic
//! snapshots, and binary path resolution for integration tests.

use regex::Regex;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the process-wide environment lock.
///
/// Workspace invariant (issue #1126): **every** mutation of the process
/// environment must hold `ENV_LOCK`, because concurrent test threads share
/// one environment and `env::set_var`/`env::remove_var` race with readers.
/// Prefer [`EnvGuard`], which acquires the lock, mutates, and restores on
/// drop. Use `env_lock` directly only when the guard's restore-on-drop
/// semantics do not fit — e.g. seeding state before a guard exists, or a
/// one-time process-wide cleanup — and keep the mutation inside the scope
/// of the returned guard.
///
/// The lock is reentrant-hostile: `EnvGuard` constructors and methods also
/// acquire it, so never call `env_lock` while an `EnvGuard` is alive.
#[must_use = "the returned guard releases the lock when dropped; bind it to a name that outlives the mutation"]
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// RAII guard that isolates environment variable mutations in tests.
///
/// Acquires the global [`env_lock`] on construction and restores all
/// modified variables to their original state on drop. This guarantees
/// serial access to the process environment across concurrent test threads.
/// Mutations that must happen while the guard is already held (flipping a
/// flag mid-test, stepping through values) go through [`EnvGuard::set`] and
/// [`EnvGuard::remove`] — never a raw `env::set_var`, which would bypass
/// the serialization the guard exists to provide.
pub struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    original_vars: Vec<(String, Option<String>)>,
}

// SAFETY: `env::set_var` / `env::remove_var` are only unsound when racing.
// ENV_LOCK serializes every process-environment mutation performed through
// this guard, and the guard restores the original state on drop.
#[allow(unsafe_code)]
impl EnvGuard {
    /// Remove the given variables from the environment, saving originals for
    /// restoration on drop.
    #[must_use]
    pub fn clean(vars: &[&str]) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut original_vars = Vec::with_capacity(vars.len());
        for &var in vars {
            let original = env::var(var).ok();
            original_vars.push((var.to_owned(), original));
            // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
            // access the environment while this guard lives.
            unsafe {
                env::remove_var(var);
            }
        }
        Self {
            _lock: lock,
            original_vars,
        }
    }

    /// Set the given variables in the environment, saving originals for
    /// restoration on drop.
    #[must_use]
    pub fn with(vars: &[(&str, &str)]) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut original_vars = Vec::with_capacity(vars.len());
        for &(var, val) in vars {
            let original = env::var(var).ok();
            original_vars.push((var.to_owned(), original));
            // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
            // access the environment while this guard lives.
            unsafe {
                env::set_var(var, val);
            }
        }
        Self {
            _lock: lock,
            original_vars,
        }
    }

    /// Set a variable while this guard already holds the environment lock,
    /// recording its current value for restoration on drop.
    ///
    /// Use this instead of a raw `env::set_var` when a test must flip a
    /// variable after the guard exists (e.g. proving a flag is captured at
    /// construction). The lock is already held by `self`, so this cannot
    /// deadlock and cannot race with any other environment mutation.
    pub fn set(&mut self, var: &str, value: &str) {
        let original = env::var(var).ok();
        self.original_vars.push((var.to_owned(), original));
        // SAFETY: ENV_LOCK is held by `self._lock` for the whole lifetime of
        // this guard, so no other thread can access the environment here.
        unsafe {
            env::set_var(var, value);
        }
    }

    /// Remove a variable while this guard already holds the environment
    /// lock, recording its current value for restoration on drop.
    ///
    /// Same serialization guarantee as [`EnvGuard::set`].
    pub fn remove(&mut self, var: &str) {
        let original = env::var(var).ok();
        self.original_vars.push((var.to_owned(), original));
        // SAFETY: ENV_LOCK is held by `self._lock` for the whole lifetime of
        // this guard, so no other thread can access the environment here.
        unsafe {
            env::remove_var(var);
        }
    }
}

// SAFETY: same ENV_LOCK serialization as the constructors; drop restores
// each variable to its captured original value (or removes it if unset).
// Restoration runs in reverse capture order so that when one variable is
// captured more than once (constructor plus `set`/`remove`), the earliest —
// true original — value is written last and wins.
#[allow(unsafe_code)]
impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (var, original) in self.original_vars.iter().rev() {
            // SAFETY: ENV_LOCK exclusivity is guaranteed — the lock is held by
            // `self._lock` until this drop completes.
            unsafe {
                match original {
                    Some(val) => env::set_var(var, val),
                    None => env::remove_var(var),
                }
            }
        }
    }
}

/// Redact the per-run temp-dir path so snapshots stay stable across machines.
#[must_use]
pub fn redact_temp_path(dir: &Path, text: &str) -> String {
    text.replace(dir.to_string_lossy().as_ref(), "<OUT_DIR>")
}

/// Redact common non-deterministic output so snapshots are stable run-to-run:
/// the temp dir, ISO-8601 log timestamps, dynamic wiremock ports, ANSI color
/// escape sequences, and source line numbers in tracing spans.
///
/// # Panics
///
/// Panics if any of the built-in redaction regular expressions fail to
/// compile. They are static literals, so this only happens if a regression
/// corrupts the pattern.
#[must_use]
pub fn redact_nondeterministic(dir: &Path, text: &str) -> String {
    let text = redact_temp_path(dir, text);
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").expect("valid ANSI regex");
    let text = ansi.replace_all(&text, "").into_owned();
    let ts = Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)")
        .expect("valid timestamp regex");
    let text = ts.replace_all(&text, "<TIMESTAMP>").into_owned();
    let port = Regex::new(r"127\.0\.0\.1:\d+").expect("valid port regex");
    let text = port.replace_all(&text, "127.0.0.1:<PORT>").into_owned();
    let line_no = Regex::new(r"(\.rs:)\d+").expect("valid line number regex");
    let text = line_no.replace_all(&text, "$1<LINE>").into_owned();
    // Normalize tracing module paths (e.g. "WARN webfang_core::cli::orchestrator:")
    // so snapshots decouple from source location and survive function moves (#462).
    let module = Regex::new(r"((?:WARN|INFO|ERROR|DEBUG|TRACE)\s+)\w+(?:::\w+)+")
        .expect("valid module regex");
    let text = module.replace_all(&text, "$1<MODULE>").into_owned();
    // Normalize tracing source file paths (e.g. "at crates/.../orchestrator.rs:<LINE>")
    // so moving a function between files does not break snapshots (#462).
    let file_path = Regex::new(r"(at\s+)\S+\.rs").expect("valid file path regex");
    file_path.replace_all(&text, "$1<FILE>.rs").into_owned()
}

/// Resolve the path to the `webfang` binary, building it on demand.
///
/// `webfang` is built by the `webfang_cli` crate (a workspace sibling),
/// so `CARGO_BIN_EXE_webfang` is only set for the crate that owns the binary.
/// This function falls back to building it via `cargo build`.
///
/// # Panics
///
/// Panics if the workspace root cannot be resolved from the crate manifest
/// directory, if `cargo` cannot be spawned, or if the build fails.
#[must_use]
pub fn webfang_path() -> PathBuf {
    if let Ok(p) = env::var("CARGO_BIN_EXE_webfang") {
        return PathBuf::from(p);
    }
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/webfang_test_utils -> crates -> workspace root (three levels up)
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .expect("resolve workspace root");
    let target_root = env::var("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| workspace_root.join("target"));
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let mut built = target_root.join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"])
        .current_dir(workspace_root)
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    built
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn env_guard_with_sets_and_restores() {
        const VAR: &str = "WEBFANG_TEST_VAR_1";
        {
            let _lock = env_lock();
            env::remove_var(VAR);
        }

        {
            let _guard = EnvGuard::with(&[(VAR, "hello")]);
            assert_eq!(env::var(VAR).unwrap(), "hello");
        }

        assert!(env::var(VAR).is_err());
    }

    #[test]
    fn env_guard_with_restores_preexisting_value() {
        const VAR: &str = "WEBFANG_TEST_VAR_2";
        {
            let _lock = env_lock();
            env::set_var(VAR, "original");
        }

        {
            let _guard = EnvGuard::with(&[(VAR, "modified")]);
            assert_eq!(env::var(VAR).unwrap(), "modified");
        }

        assert_eq!(env::var(VAR).unwrap(), "original");
        {
            let _lock = env_lock();
            env::remove_var(VAR);
        }
    }

    #[test]
    fn env_guard_clean_removes_and_restores() {
        const VAR: &str = "WEBFANG_TEST_VAR_3";
        {
            let _lock = env_lock();
            env::set_var(VAR, "present");
        }

        {
            let _guard = EnvGuard::clean(&[VAR]);
            assert!(env::var(VAR).is_err());
        }

        assert_eq!(env::var(VAR).unwrap(), "present");
        {
            let _lock = env_lock();
            env::remove_var(VAR);
        }
    }

    #[test]
    fn sequential_guards_do_not_interfere() {
        const VAR_A: &str = "WEBFANG_TEST_VAR_4";
        const VAR_B: &str = "WEBFANG_TEST_VAR_5";
        {
            let _lock = env_lock();
            env::remove_var(VAR_A);
            env::remove_var(VAR_B);
        }

        {
            let _guard = EnvGuard::with(&[(VAR_A, "a")]);
            assert_eq!(env::var(VAR_A).unwrap(), "a");
        }
        assert!(env::var(VAR_A).is_err());

        {
            let _guard = EnvGuard::with(&[(VAR_B, "b")]);
            assert_eq!(env::var(VAR_B).unwrap(), "b");
            assert!(env::var(VAR_A).is_err());
        }
        assert!(env::var(VAR_B).is_err());
    }

    /// `set`/`remove` mutate under the lock the guard already holds, and
    /// drop restores in reverse capture order so the true original wins.
    #[test]
    fn guard_set_and_remove_restore_the_true_original() {
        const VAR: &str = "WEBFANG_TEST_VAR_6";
        {
            let _lock = env_lock();
            env::set_var(VAR, "original");
        }

        {
            let mut guard = EnvGuard::clean(&[VAR]);
            assert!(env::var(VAR).is_err());
            guard.set(VAR, "flipped");
            assert_eq!(env::var(VAR).unwrap(), "flipped");
            guard.remove(VAR);
            assert!(env::var(VAR).is_err());
        }

        assert_eq!(env::var(VAR).unwrap(), "original");
        {
            let _lock = env_lock();
            env::remove_var(VAR);
        }
    }

    #[test]
    fn redact_nondeterministic_normalizes_timestamps() {
        let dir = Path::new("/tmp/test");
        let input = "at 2024-03-15T10:30:00.123+01:00 done";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "at <TIMESTAMP> done");
    }

    #[test]
    fn redact_nondeterministic_normalizes_ports() {
        let dir = Path::new("/tmp/test");
        let input = "server at 127.0.0.1:8080 started";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "server at 127.0.0.1:<PORT> started");
    }

    #[test]
    fn redact_nondeterministic_strips_ansi() {
        let dir = Path::new("/tmp/test");
        let input = "\x1b[31merror\x1b[0m: something failed";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "error: something failed");
    }

    #[test]
    fn redact_nondeterministic_replaces_temp_path() {
        let dir = Path::new("/tmp/.tmpABC123");
        let input = "wrote /tmp/.tmpABC123/output.md";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "wrote <OUT_DIR>/output.md");
    }

    #[test]
    fn redact_nondeterministic_normalizes_line_numbers() {
        let dir = Path::new("/tmp/test");
        let input = "see scrape_flow.rs:193 for details";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "see scrape_flow.rs:<LINE> for details");
    }

    #[test]
    fn redact_nondeterministic_normalizes_tracing_module_paths() {
        let dir = Path::new("/tmp/test");
        let input =
            "  2024-03-15T10:30:00+01:00  WARN webfang_core::cli::orchestrator: Unknown profile";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "  <TIMESTAMP>  WARN <MODULE>: Unknown profile");
    }

    #[test]
    fn redact_nondeterministic_normalizes_tracing_file_paths() {
        let dir = Path::new("/tmp/test");
        let input = "    at crates/webfang_core/src/cli/orchestrator.rs:42";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "    at <FILE>.rs:<LINE>");
    }
}
