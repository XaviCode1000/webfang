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
//! | `WEBFANG_MCP_DISABLE_SSRF` (exact `"1"`) | `webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV` (#1348) | MCP entry validator |
//!
//! `WEBFANG_MCP_DISABLE_SSRF` disables the MCP layer only for the exact value
//! `"1"`; every other value leaves it enabled. This does not change the
//! separate `WEBFANG_DISABLE_SSRF` contract: that variable remains
//! presence-based for the LLM extraction base-URL SSRF gate.
//!
//! # Which hatch may a test arm? (#1308, scoped by #1396)
//!
//! The #1308 rule is about the **robots chain**, whose two entry hatches —
//! `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` and `WEBFANG_MCP_DISABLE_SSRF` — are
//! read by the same fetch path. A robots test that arms only one of them
//! leaves the other layer armed, so it can pass on a phantom denial label
//! instead of the robots rules it means to exercise. For that chain the rule
//! is still absolute: use [`EnvGuard::wiremock_robots`] and never arm one of
//! *its two* hatches by hand.
//!
//! It was never a ban on single-hatch tests, and #1396 says so explicitly.
//! A suite whose subject is one layer arms exactly that layer — through a
//! named constructor, never by spelling the variable out at the call site:
//!
//! | Path under test | Constructor | Hatches armed |
//! |---|---|---|
//! | robots chain | [`EnvGuard::wiremock_robots`] | entry guard + MCP validator |
//! | MCP scrape of a loopback mock | [`EnvGuard::ssrf_hatches_off`] | entry guard + MCP validator |
//! | entry guard only (sitemap parse/discover suites) | [`EnvGuard::entry_guard_off`] | entry guard only |
//!
//! What #1308 actually forbids is an *ad-hoc* hatch: a literal env name at a
//! call site, where a rename silently desynchronizes writer and reader and
//! where nobody can see which layer the test disarmed. Adding another named
//! constructor is cheap; restating a variable is not.
//!
//! # Nesting invariant: prime env-writing `Once` init BEFORE taking `ENV_LOCK` (#1224)
//!
//! Every constructor and helper in this module acquires the process-wide
//! `ENV_LOCK`, and that lock is **not reentrant**. An `EnvGuard` holds it for
//! its entire lifetime, so the ordering of *process-wide lazy
//! initialization* is load-bearing — getting it wrong self-deadlocked the
//! whole `Tests (all features)` lane (PR #1224):
//!
//! - Init that only **reads** the env, or mutates none, is safe anywhere —
//!   e.g. an `OnceLock` that installs a port (`ensure_waf_inspector` in
//!   `application/crawler/sitemap_discovery.rs`).
//! - Init that **writes** the env — the shape is
//!   `Once::call_once(|| env_set(..))` or `OnceLock::get_or_init(|| env_set(..))`,
//!   because [`env_set`] and [`env_remove`] take `ENV_LOCK` themselves — run
//!   for the first time *inside* a guard's scope deadlocks: the init blocks on
//!   a lock the same test already holds. Under nextest every test is its own
//!   process, so the `Once` has genuinely not fired yet and the hang is total.
//!
//! The fix is to prime the `Once` while this task holds no lock, in a blocking
//! thread, and only then build the guard (live instance: `ssrf_guards_off` in
//! `crates/webfang_mcp/tests/scraping_coverage_test.rs`):
//!
//! ```ignore
//! // Prime the ONCE outside our own lock scope (#1224).
//! let _ = tokio::task::spawn_blocking(init_ssrf_disabled).await;
//! let _guards = webfang_test_utils::EnvGuard::entry_guard_off();
//! ```
//!
//! Keep the idempotent init at its original call site as well — after priming
//! it is a no-op, and it still covers harnesses that run without a guard.

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
    /// 2. `WEBFANG_MCP_DISABLE_SSRF` — the MCP validator's hatch (canonical
    ///    const:
    ///    `webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV`,
    ///    landed with #1348 and adopted here with #1370).
    ///
    /// The #1308 lesson: a robots test that arms only ONE of these two leaves
    /// the other chain layer armed, so the test can pass on a phantom denial
    /// label instead of the robots rules it means to exercise. Always use this
    /// constructor for tests that drive the robots path against a wiremock
    /// loopback literal — within that chain, never arm one of its two hatches
    /// on its own.
    ///
    /// The rule is scoped to the robots chain, not to every test (#1396): a
    /// suite whose subject is a single different layer disarms exactly that
    /// layer, through its own named constructor — [`EnvGuard::entry_guard_off`]
    /// for the literal-IP entry guard, [`EnvGuard::ssrf_hatches_off`] for an MCP
    /// loopback scrape. What stays forbidden everywhere is the ad-hoc form: a
    /// literal env variable spelled out at the call site.
    ///
    /// The guard restores both variables on drop.
    #[must_use]
    pub fn wiremock_robots() -> Self {
        Self::with(&[
            (
                webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
                "1",
            ),
            (
                webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
                "1",
            ),
        ])
    }

    /// Disarm ONLY the literal-IP SSRF entry guard (#1396).
    ///
    /// Sets `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` to the exact `"1"` the guard
    /// demands (canonical const:
    /// `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`) and nothing
    /// else: the MCP validator, the connect-time validating resolver (layer 3)
    /// and the redirect guard (layer 4) all stay armed. The guard restores the
    /// variable on drop.
    ///
    /// Use it for suites whose subject is a path that trips over exactly one
    /// layer — a wiremock loopback driven through the sitemap parser or the
    /// discovery chain (#1382): the entry guard rejects the literal seed, the
    /// other layers never see it. It is NOT the right helper for the robots
    /// chain, which consults two hatches: use [`EnvGuard::wiremock_robots`]
    /// there (#1308, see the module docs).
    ///
    /// This replaces five byte-identical local `fn entry_guard_off` helpers that
    /// had drifted into five copies of the same env name and value. The posture
    /// argument ("this suite pins X, not the guard") is per-file and stays at
    /// the call site as a comment; the mutation itself lives here, once.
    ///
    /// # Nesting (#1224)
    ///
    /// The returned guard holds the non-reentrant `ENV_LOCK` for its whole
    /// lifetime. If the harness has a process-wide `Once` init that *writes* the
    /// environment, prime it in `spawn_blocking` before calling this, or the
    /// first fetch self-deadlocks on a lock this test already holds — see the
    /// module-doc section "Nesting invariant".
    #[must_use]
    pub fn entry_guard_off() -> Self {
        Self::with(&[(
            webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        )])
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

/// Resolve the workspace root by climbing from a crate manifest directory.
///
/// Every workspace member lives at `crates/<name>`, so ONE `parent()` hop
/// reaches `crates/` and TWO reach the root that holds the virtual
/// manifest. The #1366 bug climbed three hops — one too many — landing
/// in the repo's PARENT, so the no-env fallback pointed at a `target/`
/// directory cargo never populates.
///
/// # Panics
///
/// Panics if `manifest_dir` is not at least two levels below a root —
/// impossible for a workspace member compiled in-tree.
fn workspace_root_from_manifest(manifest_dir: &str) -> PathBuf {
    PathBuf::from(manifest_dir)
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("resolve workspace root from CARGO_MANIFEST_DIR")
}

/// Pure core of [`webfang_path`]: resolve the `webfang` build-output path
/// from EXPLICIT inputs — no environment reads, no process mutation.
///
/// `bin_exe` mirrors `CARGO_BIN_EXE_webfang` (set = trusted outright),
/// `target_dir` mirrors `CARGO_TARGET_DIR` (set = output root), and
/// `manifest_dir` anchors the fallback workspace-root climb. Passing
/// `None` for both variables simulates the #1366 environment — a runner
/// with neither harness variable set — hermetically, without touching
/// the process environment.
fn resolve_webfang_path(
    bin_exe: Option<&str>,
    target_dir: Option<&str>,
    manifest_dir: &str,
) -> PathBuf {
    if let Some(p) = bin_exe {
        return PathBuf::from(p);
    }
    let workspace_root = workspace_root_from_manifest(manifest_dir);
    let target_root = match target_dir {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root.join("target"),
    };
    let mut built = target_root.join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
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
    let workspace_root = workspace_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let target_dir = env::var("CARGO_TARGET_DIR").ok();
    let built = resolve_webfang_path(None, target_dir.as_deref(), env!("CARGO_MANIFEST_DIR"));
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"])
        .current_dir(&workspace_root)
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    built
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// #1366 regression pin: with BOTH harness variables absent
    /// (`CARGO_BIN_EXE_webfang` unset — the binary belongs to a sibling crate —
    /// and `CARGO_TARGET_DIR` unset — a runner without direnv), the fallback
    /// must resolve the binary under the workspace root's own `target/`,
    /// reached by climbing exactly TWO parent hops from the crate manifest.
    /// The bug climbed three, landing one directory too high (the repo's
    /// parent), so the fallback path pointed at a `target/` directory cargo
    /// never populates — the build succeeded and the returned path was still
    /// wrong. This test simulates the absent-variable environment by passing
    /// `None` explicitly: it never mutates (or even reads) the process
    /// environment, so it cannot race ENV_LOCK or leak state into siblings.
    #[test]
    fn resolve_webfang_path_without_both_env_vars_uses_workspace_target() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let path = resolve_webfang_path(None, None, manifest);

        let root = workspace_root_from_manifest(manifest);
        assert_eq!(path, root.join("target").join("debug").join("webfang"));

        // The two-hop root must be the real workspace root: it holds the
        // virtual manifest, and it is NOT the three-hop directory the #1366
        // bug used to land in (the repo's parent).
        assert!(root.join("Cargo.toml").is_file());
        let buggy_three_hop_root = Path::new(manifest)
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .expect("three-hop chain resolves");
        assert_ne!(root, buggy_three_hop_root);
    }

    /// Cross-check the two-hop climb against the authoritative source of
    /// truth: `cargo locate-project --workspace` reports exactly where the
    /// workspace manifest lives. #1366 regressed here because the climb was
    /// hand-rolled; this pin fails if the climb and cargo ever disagree again.
    #[test]
    fn workspace_root_from_manifest_matches_cargo_metadata() {
        let root = workspace_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
        let out = std::process::Command::new(option_env!("CARGO").unwrap_or("cargo"))
            .args([
                "locate-project",
                "--workspace",
                "--message-format=plain",
                "--manifest-path",
            ])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .output()
            .expect("spawn cargo locate-project");
        assert!(out.status.success(), "cargo locate-project failed");
        // locate-project prints the workspace manifest path; strip the
        // `Cargo.toml` leaf to compare roots.
        let manifest_out = String::from_utf8(out.stdout).expect("locate-project prints UTF-8");
        let cargo_root = Path::new(manifest_out.trim_end())
            .parent()
            .expect("locate-project prints a manifest path");
        assert_eq!(
            root, cargo_root,
            "two-hop climb must land on cargo's own workspace root"
        );
    }

    /// Behavioral pins for the variable-present branches — the paths that
    /// must NOT change with #1366: an explicit `CARGO_BIN_EXE_webfang` wins
    /// outright, and an explicit `CARGO_TARGET_DIR` roots the debug output.
    #[test]
    fn resolve_webfang_path_respects_explicit_env_vars() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        assert_eq!(
            resolve_webfang_path(Some("/explicit/webfang"), None, manifest),
            PathBuf::from("/explicit/webfang")
        );

        let via_target = resolve_webfang_path(None, Some("/custom/target"), manifest);
        assert_eq!(via_target, PathBuf::from("/custom/target/debug/webfang"));
    }

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

    /// The #1396 boundary pin: `entry_guard_off` lifts the literal-IP entry
    /// guard and NOTHING else. The three sibling hatches stay untouched, so a
    /// suite that leans on this constructor cannot silently disarm the MCP
    /// validator, the resolving-time DNS guard, or the redirect guard — the
    /// failure mode #1308 was about. Restoration is on drop.
    #[test]
    fn entry_guard_off_arms_only_the_entry_hatch_and_restores_it() {
        use webfang_core::domain::ssrf_guard::{
            DISABLE_ENTRY_GUARD_ENV, DISABLE_REDIRECT_GUARD_ENV, DISABLE_VALIDATING_RESOLVER_ENV,
            WEBFANG_MCP_DISABLE_SSRF_ENV,
        };
        let hatches = [
            DISABLE_ENTRY_GUARD_ENV,
            WEBFANG_MCP_DISABLE_SSRF_ENV,
            DISABLE_VALIDATING_RESOLVER_ENV,
            DISABLE_REDIRECT_GUARD_ENV,
        ];
        {
            let _lock = env_lock();
            for hatch in hatches {
                env::remove_var(hatch);
            }
            // One sibling starts armed so "untouched" cannot be confused with
            // "set to nothing": drop must put the armed value back.
            env::set_var(DISABLE_VALIDATING_RESOLVER_ENV, "1");
        }

        {
            let _guard = EnvGuard::entry_guard_off();
            assert_eq!(
                env::var(DISABLE_ENTRY_GUARD_ENV).as_deref(),
                Ok("1"),
                "entry hatch must be armed with the exact value the guard reads"
            );
            assert!(
                env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).is_err(),
                "MCP validator hatch must stay armed (untouched)"
            );
            assert_eq!(
                env::var(DISABLE_VALIDATING_RESOLVER_ENV).as_deref(),
                Ok("1"),
                "resolver hatch must keep the value it had before the guard"
            );
            assert!(
                env::var(DISABLE_REDIRECT_GUARD_ENV).is_err(),
                "redirect hatch must stay armed (untouched)"
            );
        }

        {
            let _lock = env_lock();
            assert!(
                env::var(DISABLE_ENTRY_GUARD_ENV).is_err(),
                "entry hatch must be restored to its absent original"
            );
            assert_eq!(
                env::var(DISABLE_VALIDATING_RESOLVER_ENV).as_deref(),
                Ok("1"),
                "the sibling armed before the guard must survive it"
            );
            env::remove_var(DISABLE_VALIDATING_RESOLVER_ENV);
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
