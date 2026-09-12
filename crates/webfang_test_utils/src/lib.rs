#![deny(missing_docs)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
// Sanctioned owner of raw env mutations (#1126, #1349): every call here is
// ENV_LOCK-serialized and audited. The workspace clippy.toml configures
// `disallowed-methods` for std::env::set_var/remove_var, which fires in
// every crate once configured — this allow is the owner's exemption, the
// mirror image of the deny active in webfang_core and webfang_mcp.
#![allow(clippy::disallowed_methods)]
//! Shared test utilities for the webfang workspace.
//!
//! Provides RAII environment isolation, output redaction for deterministic
//! snapshots, and binary path resolution for integration tests.
//!
//! # SSRF test hatches — the ONE place they are documented (#1329)
//!
//! Test harnesses driving the production network path against wiremock
//! (127.0.0.1) must disarm the SSRF layers the path consults. Each hatch is
//! read with a distinct convention; all are test-only — production never
//! sets them:
//!
//! | Hatch | Canonical const | Layer it disarms |
//! |---|---|---|
//! | `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` (exact `"1"`) | `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV` | Literal-IP entry guard (SSRF choke point, #1217) |
//! | `WEBFANG_DISABLE_SSRF_REDIRECT_GUARD` | `webfang_core::domain::ssrf_guard::DISABLE_REDIRECT_GUARD_ENV` | Client redirect policy's literal-IP stop |
//! | `WEBFANG_DISABLE_SSRF_RESOLVER` | `webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV` | Connect-time validating DNS resolver |
//! | `WEBFANG_DISABLE_SSRF` (presence) | — (literal in `llm_extraction::ssrf_gate`, #703) | LLM base-URL SSRF gate |
//! | `WEBFANG_MCP_DISABLE_SSRF` | named const lands with #1348 | MCP entry validator |
//!
//! Tests that exercise the robots chain must use
//! [`EnvGuard::wiremock_robots`], which arms the entry-guard and MCP
//! hatches together — never a single hatch by hand (#1308).

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

/// Set `var` to `value` under `ENV_LOCK` with **permanent** semantics: the
/// change is NOT restored on drop — it stays for the rest of the process.
///
/// Use for one-time/permanent seeding or cleanup where restore-on-drop does
/// not fit (#1126) — e.g. harness initialization inside `Once::call_once`.
/// For atomic multi-variable setup that must restore, use
/// [`EnvGuard::with`] / [`EnvGuard::clean`]; to flip a variable mid-test
/// while already holding a guard, use [`EnvGuard::set`] /
/// [`EnvGuard::remove`].
///
/// NEVER call while an [`env_lock()`] guard or an [`EnvGuard`] scope is
/// alive: this helper acquires `ENV_LOCK` itself, and the lock is not
/// reentrant (deadlock). This is exactly why it replaces the old
/// `let _lock = env_lock(); std::env::set_var(...)` pattern — the manual
/// binding disappears, the serialization does not.
#[allow(unsafe_code)]
pub fn env_set(var: &str, value: &str) {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
    // access the environment while this lock is held.
    unsafe {
        env::set_var(var, value);
    }
}

/// Remove `var` under `ENV_LOCK` with **permanent** semantics: the variable
/// is NOT restored on drop — it stays absent for the rest of the process.
///
/// Same contract as [`env_set`]: one-time/permanent cleanup where
/// restore-on-drop does not fit (#1126); [`EnvGuard`] variants for scoped or
/// mid-test changes; never call while an [`env_lock()`] guard or an
/// [`EnvGuard`] scope is alive (the helper acquires `ENV_LOCK` itself, and
/// the lock is not reentrant).
#[allow(unsafe_code)]
pub fn env_remove(var: &str) {
    let _lock = ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
    // access the environment while this lock is held.
    unsafe {
        env::remove_var(var);
    }
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
    /// Arm BOTH SSRF hatches the robots chain can consult in tests (#1329):
    ///
    /// 1. `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` — core's literal-IP entry guard
    ///    (canonical const:
    ///    `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`), read
    ///    once per chain inside `RobotsFetcher` and again at CLI/MCP entry
    ///    points;
    /// 2. `WEBFANG_MCP_DISABLE_SSRF` — the MCP validator's hatch (named
    ///    const lands with #1348; until then the literal lives here and in
    ///    the MCP handler tests).
    ///
    /// The #1308 lesson: a robots test that arms only ONE hatch leaves the
    /// other chain layer armed, so the test can pass on a phantom denial
    /// label instead of the robots rules it means to exercise. Always use
    /// this constructor for tests that drive the robots path against a
    /// wiremock loopback literal — never arm a single hatch by hand.
    ///
    /// The guard restores both variables on drop.
    #[must_use]
    pub fn wiremock_robots() -> Self {
        Self::with(&[
            ("WEBFANG_DISABLE_SSRF_ENTRY_GUARD", "1"),
            ("WEBFANG_MCP_DISABLE_SSRF", "1"),
        ])
    }

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

    /// Lift BOTH SSRF entry hatches for an MCP wiremock-loopback harness,
    /// restoring both on drop.
    ///
    /// The double-hatch helper (#1329 paso 4, #1348): an MCP harness that
    /// scrapes a loopback wiremock literal needs BOTH
    /// `WEBFANG_MCP_DISABLE_SSRF=1` (MCP layer 1, the DNS pre-check) AND
    /// `WEBFANG_DISABLE_SSRF_ENTRY_GUARD=1` (core layer 2, the literal-IP
    /// entry guard) — see docs/ssrf-layers.md. This constructor sets both in
    /// one atomic step referencing the canonical constants in
    /// `webfang_core::domain::ssrf_guard`. Layers 3 (validating resolver) and
    /// 4 (redirect guard) stay armed.
    #[must_use]
    pub fn ssrf_hatches_off() -> Self {
        Self::with(&[
            (
                webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
                "1",
            ),
            (
                webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
                "1",
            ),
        ])
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

    /// `env_set`/`env_remove` are permanent: no restore-on-drop, and each
    /// call serializes itself under ENV_LOCK (no surrounding `env_lock`).
    #[test]
    fn env_set_and_env_remove_are_permanent() {
        const VAR: &str = "WEBFANG_TEST_VAR_7";
        {
            let _lock = env_lock();
            env::remove_var(VAR);
        }

        env_set(VAR, "permanent");
        assert_eq!(env::var(VAR).unwrap(), "permanent");

        env_set(VAR, "overwritten");
        assert_eq!(env::var(VAR).unwrap(), "overwritten");

        env_remove(VAR);
        assert!(env::var(VAR).is_err());
    }

    /// The double-hatch constructor flips both SSRF entry variables in one
    /// atomic step and restores the pre-existing (absent) state on drop.
    #[test]
    fn ssrf_hatches_off_sets_both_and_restores_both() {
        use webfang_core::domain::ssrf_guard::{
            DISABLE_ENTRY_GUARD_ENV, WEBFANG_MCP_DISABLE_SSRF_ENV,
        };
        {
            let _lock = env_lock();
            env::remove_var(WEBFANG_MCP_DISABLE_SSRF_ENV);
            env::remove_var(DISABLE_ENTRY_GUARD_ENV);
        }

        {
            let _guard = EnvGuard::ssrf_hatches_off();
            assert_eq!(env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).unwrap(), "1");
            assert_eq!(env::var(DISABLE_ENTRY_GUARD_ENV).unwrap(), "1");
        }

        assert!(env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).is_err());
        assert!(env::var(DISABLE_ENTRY_GUARD_ENV).is_err());
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
