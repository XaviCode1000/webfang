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
//! | `WEBFANG_DISABLE_SSRF` (presence) | `webfang_core::domain::ssrf_guard::LLM_SSRF_TEST_HATCH_ENV` | LLM base-URL SSRF gate |
//! | `WEBFANG_MCP_DISABLE_SSRF` (exact `"1"`) | `webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV` (#1348) | MCP entry validator |
//!
//! `WEBFANG_MCP_DISABLE_SSRF` disables the MCP layer only for the exact value
//! `"1"`; every other value leaves it enabled. This does not change the
//! separate `WEBFANG_DISABLE_SSRF` contract: that variable remains
//! presence-based for the LLM extraction base-URL SSRF gate.
//!
//! #1615 DF-E9: `WEBFANG_DISABLE_SSRF` is no longer a PRODUCTION hatch. Its
//! read is behind `#[cfg(test)]`, so it disarms the LLM gate only inside this
//! crate's own unit tests and cannot disarm it in a deployed process at all.
//! That is the one row in the table above whose scope is narrower than its
//! "all are test-only — production never sets them" preamble suggests: the
//! other hatches are production code that a test may set; this one does not
//! exist in production code. An integration test under `crates/*/tests/` links
//! `webfang_core` externally and therefore does not see `cfg(test)`, so it
//! cannot arm this row — it must not need to, and none does.
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
//! | entry guard as the SUBJECT (it must stay armed and fire) | [`EnvGuard::entry_guard_on`] | none — asserts armed |
//! | entry guard as the subject, on a loopback seed that needs layer 3 lifted | [`EnvGuard::entry_guard_on_resolver_off`] | validating resolver only; asserts entry guard armed |
//!
//! # A mutex only protects the threads that take it (#1788)
//!
//! Everything above frames `ENV_LOCK` as protecting **mutation**, and it is
//! exactly that: no two guard constructors run their `env::set_var` at the
//! same time, so a write window is never observed torn. That guarantee,
//! however, is conditional on the *reader* taking the lock too — and a reader
//! who does not take it is not protected at all.
//!
//! A test that needs the environment in its **default / armed posture** is
//! exactly such a reader. It mutates nothing, so nothing warns it that the
//! shared state it reads is mutable. Meanwhile a sibling thread may already be
//! inside an `EnvGuard::entry_guard_off()` window with the hatch set, and the
//! observer's assertions then describe the *sibling's* state, not its own.
//!
//! Under nextest this is nearly invisible: every test is its own process, so
//! the only other holder of `ENV_LOCK` would have to be in that very process,
//! which nothing else is. Under **libtest** it is a real race — every `--lib`
//! test shares one process across N threads, and the `Coverage` job runs
//! exactly that (no `--nextest`, #1788). There the four sitemap-discovery pins
//! that assert an *armed* entry guard (`..._rejects_loopback_seed_pre_socket`
//! and its three siblings) intermittently observed a disarmed guard, let an
//! RFC1918 literal through, and failed with `Http { Connect }` where the
//! contract says `CrawlError::InvalidUrl`.
//!
//! **The rule:** a test that asserts the shared state is in its default or
//! armed posture must take the same lock as one that changes it — otherwise it
//! is not protected at all. [`EnvGuard::entry_guard_on`] is that
//! lock-acquiring observer for the SSRF entry guard: it holds `ENV_LOCK` for
//! its whole lifetime, changes nothing, and asserts the hatch is not set to
//! the exact `"1"` the guard reads. Excluding the disarm window is a
//! precondition of observing the armed state, not a side effect of wanting to
//! change it.
//!
//! # Poisoning stays tolerated (#1788)
//!
//! A guard holds the lock across arbitrary test code, so any test that
//! panics while holding it poisons `ENV_LOCK` for every later test in the
//! process. Every acquisition in this module therefore uses
//! `unwrap_or_else(|poisoned| poisoned.into_inner())`: the environment is the
//! thing being serialized, and it is restored by `Drop` during the unwind
//! regardless of the mutex's poisoned flag, so refusing the lock afterwards
//! would cascade one failure into the whole suite. `EnvGuard::entry_guard_on`
//! follows the same rule — including the `catch_unwind` pin below, which
//! proves the restore-on-unwind path rather than the happy-path drop.
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

    /// Hold `ENV_LOCK` for this guard's whole lifetime and assert the
    /// literal-IP SSRF entry guard is **ARMED**, mutating nothing (#1788).
    ///
    /// The mirror image of [`EnvGuard::entry_guard_off`]: where that one
    /// disarms layer 1 and restores it on drop, this one refuses to run at all
    /// if layer 1 is currently disarmed, and changes no variable. The returned
    /// guard holds the lock until it is dropped, so no sibling test can open a
    /// disarm window for the duration of the caller's assertions.
    ///
    /// # Why
    ///
    /// `ENV_LOCK` serializes only the threads that TAKE it. The module docs
    /// frame it as protecting mutation, which is true and is not the hazard
    /// here: a test that must *observe* the armed posture mutates nothing, so
    /// nothing prompts it to take the lock — and without the lock it observes
    /// whatever a sibling thread's `entry_guard_off()` window happens to hold.
    /// The exclusion window is a precondition of observing the armed state, so
    /// an observer needs the same lock as a mutator; taking it is the whole
    /// point of this constructor.
    ///
    /// Only the exact `"1"` counts as disarmed, mirroring
    /// `reject_forbidden_literal_url`'s own read — any other value leaves the
    /// guard armed, and this constructor accepts it.
    ///
    /// # Panics
    ///
    /// Panics if `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` (canonical const:
    /// `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`) is set to
    /// the exact `"1"` while this guard is being built. The message names the
    /// variable, its observed value, and the fact that another test leaked the
    /// hatch.
    ///
    /// # Nesting (#1224, #1788)
    ///
    /// `ENV_LOCK` is **not reentrant**, and this guard holds it for its whole
    /// lifetime: never combine it with another `EnvGuard` in the same scope, or
    /// the test self-deadlocks. A test that needs a hatch lifted *as well*
    /// must use the single-step [`EnvGuard::entry_guard_on_resolver_off`]
    /// instead of stacking two guards.
    #[must_use]
    pub fn entry_guard_on() -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_entry_guard_armed();
        Self {
            _lock: lock,
            original_vars: Vec::new(),
        }
    }

    /// Keep the literal-IP entry guard ARMED (asserted) while lifting ONLY the
    /// connect-time validating resolver hatch (#1788).
    ///
    /// The posture a loopback-seed harness needs when layer 1 is the SUBJECT:
    /// the seed is spelled as a hostname (`localhost`) so layer 3 would reject
    /// the 127.0.0.1 it resolves to, and layer 1 must stay armed so a
    /// forbidden *literal* elsewhere in the chain is what fires.
    ///
    /// # Why one constructor and not two guards
    ///
    /// `ENV_LOCK` is not reentrant, so a test cannot hold both an
    /// `entry_guard_on()` observer and an `EnvGuard::with(..)` mutator: the
    /// second acquisition self-deadlocks. Folding both into one constructor is
    /// also the only shape that keeps the module's single-writer discipline
    /// (#1396): the resolver hatch's canonical name stays here, in the one
    /// place that is allowed to write it, instead of being restated at a call
    /// site where a rename would silently desynchronize writer and reader.
    /// Every other hatch stays armed — in particular the entry guard, which is
    /// asserted armed both before and after the writes below.
    ///
    /// # Panics
    ///
    /// Panics if `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` (canonical const:
    /// `webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV`) is set to
    /// the exact `"1"` either when this guard is built or after the resolver
    /// hatch is written — the second check rejects a caller that tries to smuggle
    /// the entry hatch in through another variable. On that panic the guard has
    /// already been constructed, so `Drop` still restores the resolver hatch
    /// during the unwind.
    ///
    /// # Nesting (#1224)
    ///
    /// The returned guard holds the non-reentrant `ENV_LOCK` for its whole
    /// lifetime — prime any env-*writing* process-wide `Once` in
    /// `spawn_blocking` first, exactly as for [`EnvGuard::entry_guard_off`].
    #[must_use]
    pub fn entry_guard_on_resolver_off() -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_entry_guard_armed();
        let original =
            env::var(webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV).ok();
        // SAFETY: ENV_LOCK exclusivity is guaranteed — no other thread can
        // access the environment while this guard lives.
        unsafe {
            env::set_var(
                webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
                "1",
            );
        }
        // Build the guard BEFORE re-asserting, so a panic here unwinds through
        // `Drop` and restores the resolver hatch instead of leaking it.
        let guard = Self {
            _lock: lock,
            original_vars: vec![(
                webfang_core::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV.to_owned(),
                original,
            )],
        };
        assert_entry_guard_armed();
        guard
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

/// Assert the literal-IP SSRF entry guard is ARMED, i.e. that its hatch
/// `WEBFANG_DISABLE_SSRF_ENTRY_GUARD` is not set to the exact `"1"` the
/// production read demands (#1788).
///
/// Caller contract: `ENV_LOCK` must already be held. The read is therefore
/// not itself the serialized part — the *exclusion* is. A caller that holds no
/// lock can read the hatch at any instant, including inside another test's
/// disarm window, which is the whole defect this assertion exists to surface.
fn assert_entry_guard_armed() {
    let var = webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;
    let observed = env::var(var).ok();
    assert!(
        observed.as_deref() != Some("1"),
        "{var} is set to \"1\" (observed: {observed:?}), so the SSRF entry guard is \
         DISARMED — another test leaked the hatch outside its own scope, or its \
         EnvGuard is still alive. A test whose subject IS the armed entry guard \
         must call EnvGuard::entry_guard_on() (or EnvGuard::entry_guard_on_resolver_off()) \
         so ENV_LOCK excludes the disarm window; taking no lock observes whichever \
         state a sibling thread happened to leave behind."
    );
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
/// escape sequences, source line numbers, tracing module and file paths,
/// trace/correlation UUIDs, and the OS-dependent network-failure surface.
///
/// This is THE chain: `webfang_core`'s test harness re-exports it rather
/// than keeping a second copy (#1649 — the copies drifted, and only this
/// side had the `#688` `<TRACE_ID>` rule).
///
/// # Panics
///
/// Panics if any of the built-in redaction regular expressions fail to
/// compile. They are static literals, so this only happens if a regression
/// corrupts the pattern.
#[must_use]
pub fn redact_nondeterministic(dir: &Path, text: &str) -> String {
    let text = redact_temp_path(dir, text);
    // #1777: the redaction token replaces a directory PREFIX, but the separator
    // that FOLLOWED the prefix is the OS's, and it survives verbatim — Windows
    // renders `<OUT_DIR>\typo.toml` where POSIX renders `<OUT_DIR>/typo.toml`.
    // A committed snapshot can only ever match one of them, which is how four
    // `config_default_contract_test` snapshots passed on Linux and failed on
    // `windows-latest` with nothing but this character between them.
    //
    // The rule is anchored to the token on purpose. Rewriting every backslash
    // would corrupt any message that carries one as content (a regex, a
    // literal path, a Windows-style escape) and silently fork every snapshot
    // that contains one.
    let sep = Regex::new(r"(<OUT_DIR>|<TEMP_PATH>)\\").expect("valid path separator regex");
    let text = sep.replace_all(&text, "$1/").into_owned();
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").expect("valid ANSI regex");
    let text = ansi.replace_all(&text, "").into_owned();
    let ts = Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)")
        .expect("valid timestamp regex");
    let text = ts.replace_all(&text, "<TIMESTAMP>").into_owned();
    let port = Regex::new(r"127\.0\.0\.1:\d+").expect("valid port regex");
    let text = port.replace_all(&text, "127.0.0.1:<PORT>").into_owned();
    // Normalize source line numbers in tracing spans (e.g. "scrape_flow.rs:193").
    // These shift with `#[cfg(feature = "...")]` blocks and differ across
    // feature sets, so a snapshot that baked one in would pass locally and
    // fail in a differently-featured CI job.
    let line_no = Regex::new(r"(\.rs:)\d+").expect("valid line number regex");
    let text = line_no.replace_all(&text, "$1<LINE>").into_owned();
    // Normalize tracing module paths (e.g. "WARN webfang_core::cli::orchestrator:")
    // so snapshots decouple from source location and survive function moves (#462).
    let module = Regex::new(r"((?:WARN|INFO|ERROR|DEBUG|TRACE)\s+)\w+(?:::\w+)+")
        .expect("valid module regex");
    let text = module.replace_all(&text, "$1<MODULE>").into_owned();
    // Normalize trace/correlation UUIDs emitted by log_scrape_error's trace_id
    // field (#688) so trace snapshots stay deterministic run-to-run. Moved
    // here from webfang_core's private harness copy in #1649: this is the
    // chain every consumer shares, so the rule cannot be half-applied again.
    let trace_id =
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
            .expect("valid trace id regex");
    let text = trace_id.replace_all(&text, "<TRACE_ID>").into_owned();
    // Normalize tracing source file paths (e.g. "at crates/.../orchestrator.rs:<LINE>")
    // so moving a function between files does not break snapshots (#462).
    let file_path = Regex::new(r"(at\s+)\S+\.rs").expect("valid file path regex");
    let text = file_path.replace_all(&text, "$1<FILE>.rs").into_owned();
    // INT-1 (#1631): collapse the OS-dependent connection-failure surface to
    // ONE token. Unix reports `I/O error: Connection refused (os error 111)`,
    // Windows reports our own `request timed out after 2s` (a refused
    // connection never happens there — it degrades to the request timeout), and
    // a Windows WSA refusal reads `No connection could be made ... (os error
    // 10061)`. Redaction cannot bridge a different error, so the timeout tail
    // collapses to the same token.
    //
    // The `I/O error: ` layer is part of the SAME collapse and must be
    // consumed with it: it is our `DownloadError::Io` wrapper, present only
    // when the inner error is an `io::Error`. On Unix the io::Error is what
    // carries the connection failure; on the timeout path the error is
    // `DownloadError::Timeout`, which has no such wrapper. Collapsing only the
    // tail would still leave `error de red: I/O error: <NET_ERR>` on Linux
    // against `error de red: <NET_ERR>` on Windows — same failure, different
    // snapshot. Verified against the Windows lane log, not inferred.
    //
    // The token therefore records THAT a network failure happened, not WHICH
    // one: an intentional loss, because the affected test asserts a failure is
    // mentioned rather than its kind (the same tradeoff #1645 accepted for the
    // panic payload). Pinned below against the byte-exact strings both
    // platforms produce.
    let net_err = Regex::new(
        r"(?i)(?:I/O error:\s*)?(?:(?:connection refused|connection timed out|no connection could be made[^()\n]*|an attempt to connect[^()\n]*)\s*(?:\(\s*os error\s*\d+\s*\))?|request timed out after \d+\s*s)",
    )
    .expect("valid network error regex");
    net_err.replace_all(&text, "<NET_ERR>").into_owned()
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

    /// The expected binary leaf name is platform-dependent: Windows builds
    /// `webfang.exe`. Mirrors `resolve_webfang_path`'s own
    /// `set_extension("exe")` so the expected constants stay cross-platform.
    fn expected_binary_name() -> &'static str {
        if cfg!(windows) {
            "webfang.exe"
        } else {
            "webfang"
        }
    }

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
        assert_eq!(
            path,
            root.join("target")
                .join("debug")
                .join(expected_binary_name())
        );

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
        assert_eq!(
            via_target,
            PathBuf::from("/custom/target/debug").join(expected_binary_name())
        );
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

    /// #1788 acceptance pin: the latch must be restored after a **panic**
    /// exit, not only on the happy-path drop.
    ///
    /// Every other guard test drops at the end of a scope that exits normally,
    /// so all of them exercise the same unwind. This one drives the guard out
    /// of scope through `panic!` instead, which is the only way the restore
    /// path actually runs in a real failing suite: a guard built at the top of
    /// a test, a `#[tokio::test]` assertion blowing up mid-await, and the
    /// process continuing with the next test.
    ///
    /// The assertion also pins the poisoning consequence: the panic unwinds
    /// through `Drop` *while the mutex guard is still held*, so `ENV_LOCK` is
    /// left poisoned. The post-unwind read below only succeeds because every
    /// acquisition tolerates poisoning — which is precisely the contract the
    /// module docs state, and precisely what turns one failure into a cascade if
    /// it is forgotten.
    #[test]
    fn entry_guard_restores_the_entry_hatch_after_a_panic_exit() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        use webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;

        // Capture the baseline under the lock, so the post-unwind comparison
        // is against a value no sibling could have moved underneath us.
        let original = {
            let _lock = env_lock();
            env::var(DISABLE_ENTRY_GUARD_ENV).ok()
        };

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let _guard = EnvGuard::entry_guard_off();
            assert_eq!(
                env::var(DISABLE_ENTRY_GUARD_ENV).as_deref(),
                Ok("1"),
                "the latch must be armed inside the guard, or this pin proves nothing"
            );
            panic!("deliberate panic inside the guard's scope (#1788)");
        }));

        assert!(
            outcome.is_err(),
            "the closure must have panicked, otherwise the restore-on-unwind \
             path was never taken and this test asserts nothing"
        );

        // Re-acquire the (now poisoned) lock to read: no other guard can be
        // mid-window while it is held, so this observes OUR restore and not a
        // sibling's.
        let observed = {
            let _lock = env_lock();
            env::var(DISABLE_ENTRY_GUARD_ENV).ok()
        };
        assert_eq!(
            observed, original,
            "{DISABLE_ENTRY_GUARD_ENV} was not restored after the panic exit; \
             the latch leaked and every later test would silently run with the \
             SSRF entry guard disarmed"
        );
    }

    /// #1788 triangulation for the observer side: `entry_guard_on` must
    /// REFUSE to build while the hatch is armed, and must be constructible (and
    /// mutate nothing) once it is restored. Without the refusal arm, a test
    /// could hold a guard that proves nothing; without the success arm, the
    /// constructor would be unusable.
    ///
    /// The panic is captured rather than propagated so both arms run in one
    /// test, and the leftover is cleaned up before the second arm.
    /// # Why it does not call the constructor itself
    ///
    /// `ENV_LOCK` is not reentrant, so arranging "the hatch is set" inside
    /// this test necessarily means HOLDING the lock — and calling
    /// `EnvGuard::entry_guard_on` from there would re-acquire it on the same
    /// thread and self-deadlock, hanging the whole test binary instead of
    /// failing an assertion. That is the exact hazard #1224 documents, and it
    /// is not observable as a test failure: the process just stops.
    ///
    /// So the refusal is pinned on `assert_entry_guard_armed`, the unit that
    /// decides it and the only thing `entry_guard_on` adds on top of taking the
    /// lock. The lock-acquisition half needs no separate pin: it is the same
    /// two lines `EnvGuard::entry_guard_off` already runs, and the second arm
    /// below exercises it on a real, live construction.
    #[test]
    fn entry_guard_on_refuses_a_disarmed_entry_guard() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        use webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV;

        {
            let _disarmed = EnvGuard::entry_guard_off();
            let refused = catch_unwind(AssertUnwindSafe(assert_entry_guard_armed));
            let Err(message) = refused else {
                panic!("the armed-posture assertion must refuse while the hatch is set");
            };
            let text = message
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| message.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            assert!(
                text.contains(DISABLE_ENTRY_GUARD_ENV) && text.contains("leaked the hatch"),
                "the diagnostic must name the variable and the leak, got: {text}"
            );
        }

        // Outside the disarm window the same constructor must build cleanly,
        // holding the lock without touching the environment.
        let before = {
            let _lock = env_lock();
            env::var(DISABLE_ENTRY_GUARD_ENV).ok()
        };
        {
            let _armed = EnvGuard::entry_guard_on();
            // Read directly: the guard already holds ENV_LOCK, so a second
            // acquisition here would self-deadlock (the #1224 invariant).
            assert_eq!(
                env::var(DISABLE_ENTRY_GUARD_ENV).ok(),
                before,
                "entry_guard_on must not mutate the environment"
            );
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

    /// #1777: the redaction token stands in for a directory PREFIX, but the
    /// separator that FOLLOWED the prefix is the OS's. A message carrying a
    /// real Windows path renders `<OUT_DIR>\typo.toml` where POSIX renders
    /// `<OUT_DIR>/typo.toml`, so a committed snapshot can only ever match one
    /// of them. `override_is_relative` passes on both platforms precisely
    /// because its message carries no separator.
    #[test]
    fn redact_nondeterministic_normalizes_the_separator_after_a_path_token() {
        let dir = Path::new("/tmp/test");
        let input = "no existe el archivo de configuración: <OUT_DIR>\\typo.toml";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(
            result,
            "no existe el archivo de configuración: <OUT_DIR>/typo.toml"
        );
    }

    /// The counterpart claim: a backslash that is NOT adjacent to a redaction
    /// token is content, not a separator, and must survive untouched. Without
    /// this the rule above could be widened to "replace every backslash",
    /// which would silently corrupt any snapshot whose message happens to
    /// contain one (a Windows-style escape, a regex, a literal path).
    #[test]
    fn redact_nondeterministic_leaves_a_backslash_that_is_not_after_a_token() {
        let dir = Path::new("/tmp/test");
        let input = r"patrón \N no coincide y <OUT_DIR>/ok.toml sí";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, r"patrón \N no coincide y <OUT_DIR>/ok.toml sí");
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

    /// #688 pin: `log_scrape_error` stamps a `trace_id` UUID on every error
    /// event, so any snapshot of stderr would bake a fresh UUID per run
    /// without this rule. The rule lives here (the shared chain) since
    /// #1649, so EVERY consumer redacts it — previously only
    /// `webfang_core`'s private harness copy had it, and this distributed
    /// copy did not.
    #[test]
    fn redact_nondeterministic_normalizes_trace_ids() {
        let dir = Path::new("/tmp/test");
        let input =
            "error de red: failed trace_id=3f2504e0-4f89-11d3-9a0c-0305e82c3301 correlation=ABCDEF01-2345-6789-ABCD-EF0123456789";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(
            result,
            "error de red: failed trace_id=<TRACE_ID> correlation=<TRACE_ID>"
        );
    }

    #[test]
    fn redact_nondeterministic_normalizes_tracing_file_paths() {
        let dir = Path::new("/tmp/test");
        let input = "    at crates/webfang_core/src/cli/orchestrator.rs:42";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "    at <FILE>.rs:<LINE>");
    }

    /// INT-1 (#1631) pin, Unix shape: the errno is OS text (`os error 111`
    /// on Linux, `61` on macOS) and a snapshot must not bake it in. The
    /// surrounding Spanish error prefix is product-owned and survives; the
    /// `I/O error: ` layer does NOT, because the Windows timeout path has no
    /// such layer and keeping it made the two platforms disagree — see
    /// `redact_nondeterministic_makes_both_platforms_agree`.
    #[test]
    fn redact_nondeterministic_collapses_unix_connection_refused() {
        let dir = Path::new("/tmp/test");
        let input = "error de red: I/O error: Connection refused (os error 111)";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "error de red: <NET_ERR>");
    }

    /// INT-1 (#1631) pin, Windows shape: the refusal is WSA prose plus the
    /// same errno parenthetical, so the WHOLE phrase must collapse — a
    /// narrower rule that only replaced the number would still leave
    /// Windows-only text in a Linux-authored snapshot.
    #[test]
    fn redact_nondeterministic_collapses_windows_wsa_prose() {
        let dir = Path::new("/tmp/test");
        let input = "No connection could be made because the target machine actively refused it. (os error 10061)";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "<NET_ERR>");
    }

    /// INT-1 (#1631) pin, Windows timeout-degradation shape: an unreachable
    /// host on Windows never refuses, it times out, so the snapshot has to
    /// see the SAME token the Unix refusal produces.
    #[test]
    fn redact_nondeterministic_collapses_windows_timeout_degradation() {
        let dir = Path::new("/tmp/test");
        let input = "error de red: request timed out after 2s";
        let result = redact_nondeterministic(dir, input);
        assert_eq!(result, "error de red: <NET_ERR>");
    }

    /// The pin that would have caught the `I/O error: ` layer the first two
    /// pins missed. Both strings below are copied BYTE-EXACT from the stderr
    /// each platform produced for `unreachable_host_stderr_mentions_failure`
    /// (Windows lane, run 36356048341) — not reconstructed from memory:
    ///
    /// ```text
    /// linux:   error de red: I/O error: Connection refused (os error 111)
    /// windows: error de red: request timed out after 2s
    /// ```
    ///
    /// Written as two separate pins, they both passed while the RULE was wrong,
    /// because each pin asserted only its own shape: the Unix pin expected the
    /// `I/O error: ` layer to SURVIVE, and the Windows pin had no layer to
    /// begin with. The defect was only visible when the two are compared, so
    /// this asserts the comparison itself — the whole point of the collapse.
    #[test]
    fn redact_nondeterministic_makes_both_platforms_agree() {
        let dir = Path::new("/tmp/test");
        let unix = redact_nondeterministic(
            dir,
            "Failed to scrape http://x/: error de red: I/O error: Connection refused (os error 111)",
        );
        let windows = redact_nondeterministic(
            dir,
            "Failed to scrape http://x/: error de red: request timed out after 2s",
        );
        assert_eq!(unix, windows, "the two platforms must redact identically");
        assert_eq!(
            unix, "Failed to scrape http://x/: error de red: <NET_ERR>",
            "and both must land on the documented token"
        );
    }
}
