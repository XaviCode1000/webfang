//! Shared CLI behavioral test harness for the `webfang` binary.
//!
//! `#![allow(dead_code, unused_imports)]`: this file is included via `#[path]`
//! from fifteen separate test crates, each of which uses a different subset of
//! the helpers. Items (and the re-exports below) unused by a given crate would
//! otherwise trip `-D warnings`; gating them here is intentional — the same
//! two lints the `common` module already gates for the crates that reach this
//! file through `common/mod.rs`.
#![allow(dead_code, unused_imports)]
//!
//! Centralized helpers used by the `webfang_core` test binaries that include
//! this file, so the `webfang_path()` resolver, the `BehavioralTest`
//! mock-server/temp-dir harness, and the output-redaction helpers live in
//! exactly one place.
//!
//! The snapshot-assertion wrappers (`assert_snapshot_redacted` /
//! `assert_snapshot_plain`) are intentionally NOT defined here: insta derives a
//! snapshot's on-disk location from the module path where `assert_snapshot!`
//! expands, so those wrappers must stay at each test crate's root module to
//! preserve existing snapshot folders.
//!
//! Include this file from a test crate via:
//!
//! ```ignore
//! #[path = "../common/cli_harness.rs"]
//! mod common;
//! pub use crate::common::{cmd, redact_nondeterministic, webfang_path, BehavioralTest};
//! ```

use assert_cmd::Command;
use insta::assert_snapshot;
use std::path::Path;
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Resolve the path to the `webfang` binary, building it on demand.
///
/// `webfang` is built by the `webfang_cli` crate (a workspace sibling),
/// so `assert_cmd::cargo_bin` cannot locate it from `webfang_core` tests
/// — `CARGO_BIN_EXE_webfang` is only set for the crate that owns the binary.
///
/// Hermeticity (#302): the binary is ALWAYS built with the EXACT feature set
/// active in the test crate — never reused on the sole basis of
/// `target/debug/webfang` existing. Features are derived individually via
/// `cfg!()` so the binary matches the test's own configuration.
pub(crate) fn webfang_path() -> std::path::PathBuf {
    // #1697: resolve ONCE per process. This function shells out to
    // `cargo build`, so without the cache every `cmd()` in a test process
    // re-invoked the compiler — cheap when it is a no-op, but the FIRST call
    // in a fresh target dir pays the whole build, inside whatever the caller
    // was timing. Measured on this change: cold 40.2 s (the test's own 20 s
    // budget, twice, then a 25 s pass), warm 0.319 s for the same test.
    static PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(resolve_webfang_path).clone()
}

fn resolve_webfang_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return std::path::PathBuf::from(p);
    }
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/webfang_core -> workspace root (two levels up)
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve workspace root");
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let mut build_args = vec!["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"];
    // Derive the exact feature set from the test crate's active features.
    // Each `cfg!()` is a compile-time constant — zero runtime cost.
    //
    // #1813 T4: the `images` / `documents` arms are GONE. Those markers are
    // deleted from `webfang_core` and `webfang_cli`, so forwarding them here
    // would make every behavioral test abort with cargo's "none of the
    // selected packages contains these features". They gated nothing reachable
    // from the binary — only a `pub` MIME helper with no callers — so the
    // child build compiles the same code with and without them. Note this list
    // is NOT the crate's whole feature set, and no `--no-default-features` is
    // passed: the child always builds on top of `webfang_cli`'s defaults.
    //
    // ADR-0004 paso 6: the `ai` arm is GONE too, and for a stronger reason
    // than the two above. The `ai` feature gated the whole local-ONNX stack
    // (`webfang_ai` + `ort` + `hf-hub`), and the src side of this branch is
    // fully de-gated: the semantic cleaner runs over the remote
    // `EmbeddingPort` in every configuration. Forwarding `ai` would make the
    // child build depend on a feature the manifests are about to lose, and
    // the test binary would then be the only thing keeping a dead
    // configuration alive. The binary under test is therefore built WITHOUT
    // `ai` — the same configuration a user gets.
    let mut active_features = Vec::new();
    if cfg!(feature = "adaptive-selectors") {
        active_features.push("adaptive-selectors");
    }
    if cfg!(feature = "mcp") {
        active_features.push("mcp");
    }
    if cfg!(feature = "persistence") {
        active_features.push("persistence");
    }
    if cfg!(feature = "console") {
        active_features.push("console");
    }
    if cfg!(feature = "dev-tracing") {
        active_features.push("dev-tracing");
    }
    let features_arg;
    if !active_features.is_empty() {
        features_arg = active_features.join(",");
        build_args.push("--features");
        build_args.push(&features_arg);
    }
    let status = std::process::Command::new(cargo)
        .args(&build_args)
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    let mut built = if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        std::path::PathBuf::from(target_dir)
            .join("debug")
            .join("webfang")
    } else {
        workspace_root.join("target").join("debug").join("webfang")
    };
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
}

/// Shared binary command builder for tests that don't need a mock server.
pub(crate) fn cmd() -> Command {
    sanitize_env(Command::new(webfang_path()))
}

/// Wall-clock budget for one spawned-binary attempt whose real latency is
/// milliseconds and whose failure mode is a hang (#1697).
///
/// The observed healthy run on `windows-latest` takes 3.05 s, so this leaves
/// a ~6x margin — wide enough that a loaded runner does not trip it, tight
/// enough that a genuinely stuck attempt is abandoned long before nextest's
/// 180 s `terminate-after`.
pub(crate) const SPAWN_LATENCY_BUDGET: Duration = Duration::from_secs(20);

/// #1697: run a spawned-binary assertion under a wall-clock budget THIS
/// harness enforces, retrying once before it declares failure.
///
/// `assert_cmd`'s `.timeout()` is not a bound a test process can rely on: on
/// `windows-latest` `batch_empty_file_exits_64` set it to 5 s and the test
/// still ran until nextest killed it at 180 s. With `--retries 2`, that cost
/// the first attempt the whole `terminate-after` and then passed in 3 s —
/// which is why one flaky test consumed 26% of the `Integration tests` step's
/// wall clock while reporting itself as a plain FLAKY.
///
/// The assertion therefore runs on a worker thread and the caller waits with
/// `recv_timeout`, so an overrunning attempt is *abandoned* instead of being
/// allowed to hold the test process open. One retry, then a panic carrying
/// BOTH measured latencies — a real regression overruns every attempt and so
/// still fails, with the numbers that say by how much.
///
/// The abandoned thread outlives the budget but not the test: nextest gives
/// each test its own process, so whatever is still blocked dies with it.
///
/// The retry loop itself lives in [`run_under_budget`], so the budget property
/// can be asserted on the waits THIS harness measured rather than on a caller's
/// wall clock — see [`SpawnOutcome`] (#2063).
pub(crate) fn assert_spawn_within<F>(budget: Duration, what: &str, attempt: F)
where
    F: Fn() -> Result<(), String> + Send + Sync + 'static,
{
    match run_under_budget(budget, attempt) {
        SpawnOutcome::Passed => {},
        SpawnOutcome::Reported(failure) => panic!("{what}: {failure}"),
        SpawnOutcome::Disconnected(attempt_no) => {
            panic!("{what}: the attempt ended without a verdict (attempt {attempt_no})")
        },
        SpawnOutcome::Overran(overruns) => panic!(
            "{what}: exceeded its {budget:?} budget on 2 consecutive attempts \
             (latencies {overruns:?}). A regression would overrun every attempt, \
             so this failure is real — see #1697 for the windows-latest flake this \
             budget exists to bound."
        ),
    }
}

/// Sealed verdict of [`run_under_budget`], which [`assert_spawn_within`] turns
/// into a panic.
///
/// The budget loop is its own function so the budget property can be asserted
/// on the waits the harness MEASURED, instead of on a caller's wall clock: the
/// clock also pays for two thread spawns, the panic hook, backtrace
/// symbolization and unwinding. On `windows-latest` that overhead measured
/// 398 ms against 211 ms of real waits, so a bound derived from the waits alone
/// failed while every wait was inside its budget (#2063).
#[derive(Debug)]
enum SpawnOutcome {
    /// The attempt reported success inside its budget.
    Passed,
    /// The attempt produced its own failure verdict.
    Reported(String),
    /// The attempt ended without a verdict (its sender was dropped).
    Disconnected(u8),
    /// Both attempts overran, carrying each attempt's measured wait.
    Overran(Vec<Duration>),
}

/// Run `attempt` up to twice under a wall-clock budget, abandoning each
/// overrunning attempt and recording how long it was allowed to run.
fn run_under_budget<F>(budget: Duration, attempt: F) -> SpawnOutcome
where
    F: Fn() -> Result<(), String> + Send + Sync + 'static,
{
    let attempt = std::sync::Arc::new(attempt);
    // #1697: the budget covers the SPAWN, never a compiler invocation.
    // `webfang_path()` falls back to `cargo build` when `CARGO_BIN_EXE_webfang`
    // is unset — which it always is here, since the binary is owned by the
    // sibling crate `webfang_cli`. Resolving it before the clock starts keeps a
    // cold target directory from being charged to the spawn latency.
    let _ = webfang_path();
    let mut overruns: Vec<Duration> = Vec::new();
    for attempt_no in 1..=2u8 {
        let started = Instant::now();
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let worker = std::sync::Arc::clone(&attempt);
        std::thread::spawn(move || {
            let _ = tx.send(worker());
        });
        match rx.recv_timeout(budget) {
            Ok(Ok(())) => return SpawnOutcome::Passed,
            Ok(Err(failure)) => return SpawnOutcome::Reported(failure),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => overruns.push(started.elapsed()),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return SpawnOutcome::Disconnected(attempt_no)
            },
        }
    }
    SpawnOutcome::Overran(overruns)
}

/// The env names a spawned binary must never inherit: `WEBFANG_*` (every flag
/// override and the crash-point arming var) plus `AI_MODEL_ID`.
///
/// Split from [`strip_poisoned_env`] because the signal harness spawns through
/// `std::process::Command` rather than `assert_cmd::Command`, and the two
/// types share no deref: the LIST is the policy and lives here once, while each
/// caller removes it from the command type it holds.
pub(crate) fn poisoned_env_names() -> Vec<String> {
    std::env::vars()
        .filter(|(k, _)| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
        .map(|(k, _)| k)
        .collect()
}

/// Strip `WEBFANG_*` / `AI_MODEL_ID` from a spawned binary command so runs
/// stay hermetic even when CI bug-discovery workflows poison the environment.
pub(crate) fn strip_poisoned_env(cmd: &mut Command) {
    for key in poisoned_env_names() {
        cmd.env_remove(&key);
    }
}

/// Remove all `WEBFANG_*` and `AI_MODEL_ID` env vars from a command so tests
/// are hermetic even when CI bug-discovery workflows poison the environment,
/// and point `XDG_CACHE_HOME` at a fresh per-invocation temp directory.
///
/// The AI model env names (`WEBFANG_AI_MODEL_ID`, `AI_MODEL_ID`) are kept in
/// the strip list even though nothing reads them any more — the migration
/// shims that used to are retired (issue #2119) — because a name that is
/// stripped today must not start being honoured by a future flag, and
/// removing it from the list would make that a silent difference.
///
/// Hermetic cache: checkpointing is opt-in (F-01: default flags resolve to
/// `Disabled`), but an explicit `--resume`/`--state-dir` run still loads
/// and saves checkpoint state under the cache. Parallel behavioral tests
/// all run against `127.0.0.1` wiremock servers
/// with ephemeral ports, and the OS reuses recently freed ports within one
/// test-binary run, so a visited URL recorded by one test
/// (`http://127.0.0.1:<port>/page1`) can collide with a later test whose
/// server drew the same port: the engine then treats that test's seed or
/// pages as already visited and crawls nothing ("expected /page1 to be
/// crawled, got 0"). The resume state file is worse — it is keyed by host
/// WITHOUT the port (`127.0.0.1.json`), so it collides across every
/// wiremock test. Giving each spawned binary its own cache base keeps both
/// state files per-test without changing any production default.
fn sanitize_env(mut cmd: Command) -> Command {
    strip_poisoned_env(&mut cmd);
    // SSRF entry-guard allowance (F-06 + F-32, #1217): every behavioral
    // mock binds 127.0.0.1 — a forbidden literal the production request
    // path now rejects — so the harness disarms ONLY the entry layer for
    // spawned binaries. Production never sets this; the entry-guard
    // regression test opts back out with `.env_remove(...)`.
    cmd.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    cmd.env("XDG_CACHE_HOME", hermetic_cache_dir());
    // Config gets the same treatment, and for a stronger reason than the
    // cache: the CLI resolves its config file through `WEBFANG_CONFIG`
    // (`cli::config::load_config_defaults`), so pointing that at a file that
    // carries no settings means a spawned binary can never read the
    // developer's real `~/.config/webfang/config.toml`, and the run cannot
    // depend on the operator's machine. A per-test `.env("WEBFANG_CONFIG", …)`
    // applied after `cmd()` still wins: both writes land in the inner
    // `std::process::Command` env map, and the last write for a key is the one
    // applied at spawn.
    cmd.env("WEBFANG_CONFIG", hermetic_empty_config_file());
    cmd
}

/// Unique cache base directory for one spawned-binary invocation.
///
/// Created eagerly so no code path observes a missing parent. The `Command`
/// cannot own the directory's lifetime, so it is intentionally left for the
/// OS temp cleanup to reclaim.
fn hermetic_cache_dir() -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("webfang-test-cache-{}-{n}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// An EMPTY config file for one spawned-binary invocation — the hermetic twin
/// of [`hermetic_cache_dir`].
///
/// This used to hand back a path that was never created, which was safe only
/// while an explicit `WEBFANG_CONFIG` degraded silently. Since #1659 an
/// override is a statement of intent: `load_config_defaults` rejects a
/// relative, missing, or unreadable one instead of falling back to defaults,
/// so pointing every spawned binary at an absent path would fail every test
/// in the suite at step 5 of startup.
///
/// An empty file is the equivalent that survives the strict contract.
/// `ConfigDefaults` is `#[serde(default)]`, so empty TOML parses to exactly
/// the same all-`None` defaults the absent file used to produce — hermetic,
/// without depending on a fallback the loader no longer offers. The
/// alternative, pointing at a *valid* config, would leak real settings into
/// every test; that is why the file is empty rather than representative.
/// `strip_poisoned_env` removes the developer's own `WEBFANG_CONFIG` on the
/// way in, so nothing from outside the process can land here either.
fn hermetic_empty_config_file() -> std::path::PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "webfang-test-empty-config-{}-{n}.toml",
        std::process::id()
    ));
    std::fs::write(&path, "").expect("create empty hermetic config.toml");
    path
}

// ---------------------------------------------------------------------------
// Child-process signal harness (#1292, #2244)
// ---------------------------------------------------------------------------
//
// Every test that INTERRUPTS a real child needs the same five pieces: a spawn
// carrying the harness's hermetic env, a handle that cannot leak the child, a
// deterministic wait for the k-th page fetch, a POSIX signal without `unsafe`,
// and a bounded wait for the exit. They used to live twice — in
// `behavioral/cli/signal_lifecycle_test.rs` and `tests/crash_matrix_test.rs` —
// until the duplication ratchet caught the copy (PR #2268).

/// How long a child may take to exit after the signal before the test fails.
pub(crate) const EXIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound on the Drop-time reap. `Child::kill` is SIGKILL on Unix and
/// TerminateProcess on Windows — both immediate — so a child still unreaped
/// after this window is not going to be. The bound is the point: a stuck
/// child must not block the panic unwind that is running the cleanup and
/// hide the assertion that caused it. Leaking a zombie beats hanging a test.
pub(crate) const REAP_TIMEOUT: Duration = Duration::from_secs(5);

/// Poll interval while waiting for request counts and child exit.
pub(crate) const POLL: Duration = Duration::from_millis(50);

/// Per-page response delay: slows the crawl enough that a signal always
/// lands mid-run, without making the test slow.
pub(crate) const PAGE_DELAY: Duration = Duration::from_millis(200);

/// Upper bound on the wait for the k-th page fetch: a crawl that never
/// reaches it is stalled, and saying so is more useful than hanging.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Count GETs to real content pages (anything that is not `/robots.txt` and
/// not the sitemap) — the deterministic "page fetch" metric behind every
/// signal assertion: the trigger is a request wiremock OBSERVED, never a
/// sleep.
pub(crate) async fn count_page_fetches(server: &MockServer) -> usize {
    server
        .received_requests()
        .await
        .expect("request log")
        .iter()
        .filter(|r| {
            let p = r.url.path();
            p != "/robots.txt" && !p.ends_with("sitemap.xml")
        })
        .count()
}

/// Poll until the server has seen at least `k` page fetches (the
/// deterministic signal trigger), bounded by [`FETCH_TIMEOUT`].
pub(crate) async fn wait_for_page_fetches(server: &MockServer, k: usize) {
    let deadline = Instant::now() + FETCH_TIMEOUT;
    loop {
        if count_page_fetches(server).await >= k {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "server never reached {k} page fetches — crawl stalled"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Send `signal` to `child` via the system `kill` binary: the crate forbids
/// `unsafe`, so there is no `libc::kill`, and the OS-level trigger goes
/// through `kill(1)` instead.
pub(crate) fn send_signal(child: &Child, signal: &str, what: &str) {
    let status = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .arg(child.id().to_string())
        .status()
        .unwrap_or_else(|e| panic!("{what}: invoking kill failed: {e}"));
    assert!(
        status.success(),
        "{what}: kill -{signal} {} must succeed",
        child.id()
    );
}

/// A `Child` that dies with its handle. `std::process::Child` has no
/// kill-on-drop (that is `tokio::process::Command`'s), and a dropped `Child`
/// does not reap: a crawl still running when an assertion panics survives the
/// test and nextest reports it LEAKY, burying the real failure under process
/// noise — the exact `FL+LK` shape recorded in #1631. `Deref` keeps
/// `send_signal`/`wait_exit` reading the handle they always did.
///
/// The `Drop` reaps as well as kills, because a kill request alone leaves a
/// zombie occupying the process table, and nextest's leak check looks there.
pub(crate) struct KillOnDrop(pub Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // Disarm BEFORE signalling. A reaped child's PID is free for reuse,
        // and `Child::kill` targets the stored numeric PID, so an
        // unconditional kill here could SIGKILL an unrelated process that
        // inherited it — on a shared CI runner that is someone else's job.
        // `try_wait` reports `Some` once the child is reaped (std caches the
        // status), which is the only safe way to learn that.
        if self.0.try_wait().is_ok_and(|status| status.is_some()) {
            return;
        }

        // Known unreaped: request termination, then reap. BOUNDED, because
        // this runs while a panic unwinds and a child that ignores the kill
        // must not block the unwind and hide the assertion that caused it.
        // Both results are deliberately ignored: on the happy path the kill
        // is a no-op, and on the failure path the cleanup is best effort.
        let _ = self.0.kill();
        let deadline = Instant::now() + REAP_TIMEOUT;
        while Instant::now() < deadline {
            match self.0.try_wait() {
                // Reaped, or unanswerable — either way, stop waiting.
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => std::thread::sleep(POLL),
            }
        }
    }
}

impl std::ops::Deref for KillOnDrop {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for KillOnDrop {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Spawn the real binary with piped output and the harness's hermetic env, as
/// a bare `std::process::Command` — needed because the test must hold the
/// child handle to deliver the signal.
pub(crate) fn spawn_webfang(args: &[String], cache_dir: &Path, what: &str) -> KillOnDrop {
    spawn_webfang_with(args, cache_dir, what, |_| {})
}

/// [`spawn_webfang`] plus a hook to adjust the command before spawn — for the
/// rows that need their own stdio or an extra env var (the crash matrix arms
/// `WEBFANG_CRASH_AT` this way, AFTER the strip, so the parent process can
/// never poison the armed point).
///
/// Hermeticity matches [`sanitize_env`]: the poisoned names are stripped, the
/// cache is per-invocation, and `WEBFANG_CONFIG` points at an empty file. The
/// config matters for the same reason it does there — without a replacement
/// the child falls back to the developer's `~/.config/webfang/config.toml`,
/// and a real file enabling `clean-ai` would route the export through the AI
/// drain, a path these tests must not depend on.
pub(crate) fn spawn_webfang_with(
    args: &[String],
    cache_dir: &Path,
    what: &str,
    configure: impl FnOnce(&mut std::process::Command),
) -> KillOnDrop {
    let mut c = std::process::Command::new(webfang_path());
    c.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for key in poisoned_env_names() {
        c.env_remove(&key);
    }
    c.env("XDG_CACHE_HOME", cache_dir);
    c.env("WEBFANG_CONFIG", hermetic_empty_config_file());
    // SSRF entry-guard allowance (F-06 + F-32, #1217): the mock binds
    // 127.0.0.1, a forbidden literal — disarm ONLY the entry layer.
    c.env(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
    configure(&mut c);
    KillOnDrop(
        c.spawn()
            .unwrap_or_else(|e| panic!("{what}: spawn failed: {e}")),
    )
}

/// Wait for the signalled child to exit; fail (after SIGKILL) if it exceeds
/// [`EXIT_TIMEOUT`] — a shutdown must not hang. Returns the exit status plus
/// the captured stderr, so a death by an unexpected signal can name its cause
/// instead of leaving a bare wait status.
///
/// Takes the [`KillOnDrop`] handle by value: `Deref` covers the method calls
/// below, but a by-value argument needs the guard itself so the kill-on-drop
/// guarantee survives this call too.
pub(crate) fn wait_exit(mut child: KillOnDrop, what: &str) -> (ExitStatus, String) {
    use std::io::Read;

    let deadline = Instant::now() + EXIT_TIMEOUT;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(status) => {
                // The child is reaped and its stderr write-end is closed, so
                // draining the pipe to EOF cannot block.
                let mut stderr = String::new();
                child
                    .stderr
                    .take()
                    .expect("stderr pipe")
                    .read_to_string(&mut stderr)
                    .unwrap_or_default();
                return (status, stderr);
            },
            None => {
                assert!(
                    Instant::now() < deadline,
                    "{what}: child did not exit within {EXIT_TIMEOUT:?} — shutdown hung"
                );
                std::thread::sleep(POLL);
            },
        }
    }
}

/// Mount a single catch-all page on the test's mock server.
///
/// One mount serves both directions, which is why it is a shared helper: a run
/// that PASSES validation gets a hermetic, instant success target instead of
/// the real network, AND a run that should have been rejected still resolves
/// its mock if the rejection regresses — observed as exit 0 rather than as a
/// network error, which is the failure mode that hides.
pub(crate) async fn mount_single_page_site(t: &BehavioralTest) {
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><article><h1>Page</h1>\
             <p>Substantive content long enough to clear the fifty character \
             minimum content guard comfortably.</p></article></body></html>",
        ))
        .mount(&t.server)
        .await;
}

/// Shared test harness: one mock server + one temp output directory.
pub(crate) struct BehavioralTest {
    pub server: wiremock::MockServer,
    pub out: tempfile::TempDir,
}

impl BehavioralTest {
    /// Spin up a fresh mock server and temp directory.
    pub async fn new() -> Self {
        Self {
            server: wiremock::MockServer::start().await,
            out: tempfile::TempDir::new().expect("create temp output dir"),
        }
    }

    /// Build a `Command` for the `webfang` binary with `--url` and
    /// `--output` pre-filled to this harness's server and temp dir.
    pub fn scraper_cmd(&self) -> assert_cmd::Command {
        let mut cmd = sanitize_env(Command::new(webfang_path()));
        cmd.arg("--url")
            .arg(self.server.uri())
            .arg("--output")
            .arg(self.out.path());
        cmd
    }

    /// Recursively find all files matching the given extension inside the
    /// output directory (files live in domain subdirs).
    pub fn find_files(&self, ext: &str) -> Vec<std::path::PathBuf> {
        walkdir::WalkDir::new(self.out.path())
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension().is_some_and(|x| x == ext))
            .map(|e| e.path().to_path_buf())
            .collect()
    }

    /// Read the first `.md` file found in the output directory.
    /// Panics if no `.md` file exists.
    pub fn read_md_content(&self) -> String {
        let md_files = self.find_files("md");
        assert!(
            !md_files.is_empty(),
            "expected at least one .md file in output"
        );
        std::fs::read_to_string(&md_files[0]).expect("read .md file")
    }

    /// Build a `Command` with `--elastic` flag, `--url`, and a fresh SQLite
    /// temp directory for the elastic output path.
    pub fn elastic_cmd(&self) -> assert_cmd::Command {
        let mut cmd = sanitize_env(Command::new(webfang_path()));
        cmd.arg("--elastic")
            .arg("--url")
            .arg(self.server.uri())
            .arg("--output")
            .arg(self.out.path());
        cmd
    }

    /// Build a `Command` with `--resume` flag, `--url`, and the existing
    /// output directory (resume reads from a prior crawl state).
    pub fn resume_cmd(&self) -> assert_cmd::Command {
        let mut cmd = sanitize_env(Command::new(webfang_path()));
        cmd.arg("--resume")
            .arg("--url")
            .arg(self.server.uri())
            .arg("--output")
            .arg(self.out.path());
        cmd
    }

    /// Build a `Command` with `--resume` and a custom `--state-dir`, plus
    /// `--url` and `--output`. The caller supplies a fresh temp dir for the
    /// state so tests never read or write the shared default cache
    /// (`~/.cache/webfang/state`), which collides across wiremock runs because
    /// the state file is keyed by host without the port (`127.0.0.1.json`).
    pub fn state_dir_cmd(&self, state_dir: &Path) -> assert_cmd::Command {
        let mut cmd = sanitize_env(Command::new(webfang_path()));
        cmd.arg("--resume")
            .arg("--state-dir")
            .arg(state_dir)
            .arg("--url")
            .arg(self.server.uri())
            .arg("--output")
            .arg(self.out.path());
        cmd
    }

    /// Count the JSONL records in the export file at `relative` (relative to
    /// the output dir). FIX-0 (#1235): the audit instrument — record
    /// cardinality against the REAL binary, so a writer that duplicates or
    /// drops records fails the test instead of the user finding it in a
    /// downstream RAG pipeline (AUDIT-01 F-02: batch wrote every record
    /// twice; nothing in the suite counted).
    ///
    /// Every non-empty line is one record: the JSONL writer emits one record
    /// per line and the exporter has no multi-line records.
    pub fn record_count(&self, relative: &str) -> usize {
        let path = self.out.path().join(relative);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        body.lines().filter(|l| !l.trim().is_empty()).count()
    }

    /// Register a request-counting mock: responds to `count` GETs on `route`
    /// with `status`/`body`, and FAILS the test on drop if the binary made a
    /// different number of requests (wiremock verifies expectations on
    /// `MockGuard` drop).
    ///
    /// FIX-0 (#1235): the audit instrument for request-count invariants —
    /// the fixture server counted requests at `GET /state`; in tests the
    /// equivalent is a wiremock expectation held by a guard the caller must
    /// keep alive for the whole command.
    ///
    /// Returns the guard: bind it (`let _guard = ...`), NOT `let _ =` — a
    /// dropped guard verifies immediately, before the command runs.
    pub async fn mount_counting(
        &self,
        route: &str,
        count: u64,
        status: u16,
        body: &str,
    ) -> wiremock::MockGuard {
        Mock::given(method("GET"))
            .and(wm_path(route))
            .respond_with(ResponseTemplate::new(status).set_body_string(body))
            .expect(count)
            .mount_as_scoped(&self.server)
            .await
    }
}

/// Register a wiremock mock that responds to GET on the given relative path
/// with an XML sitemap body and `200 OK`.
///
/// The `url` should be the full mock-server URI (e.g. `server.uri()`), and
/// `xml_body` is the raw XML string to return.
pub(crate) async fn mock_sitemap(server: &wiremock::MockServer, url: &str, xml_body: &str) {
    // Extract the path portion from the URL (everything after the host:port)
    let path_part = url.splitn(4, '/').nth(3).unwrap_or("sitemap.xml");

    Mock::given(method("GET"))
        .and(wm_path(format!("/{path_part}")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(xml_body)
                .insert_header("Content-Type", "application/xml"),
        )
        .mount(server)
        .await;
}

/// Register a wiremock mock that responds to GET on `/robots.txt` with the
/// given body and `200 OK`.
pub(crate) async fn mock_robots(server: &wiremock::MockServer, robots_body: &str) {
    Mock::given(method("GET"))
        .and(wm_path("/robots.txt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(robots_body)
                .insert_header("Content-Type", "text/plain"),
        )
        .mount(server)
        .await;
}

/// The output-redaction chain lives in `webfang_test_utils` — the crate
/// every `webfang_*` test target already depends on — and is re-exported here
/// so `crate::redact_nondeterministic` keeps resolving for the test crates
/// that reach this file via `#[path]`.
///
/// #1649: this file used to carry its OWN copy of both functions, and the two
/// copies drifted (only the local one had the `#688` `<TRACE_ID>` rule), so the
/// same output was sanitized differently depending on which harness recorded
/// it. `redact_temp_path` is re-exported alongside the chain because the chain
/// is built on it; `redact_nondeterministic` is the one every consumer uses.
pub(crate) use webfang_test_utils::{redact_nondeterministic, redact_temp_path};

/// Snapshot a stderr blob with the harness temp dir and other
/// non-deterministic output redacted.
///
/// BOTH redaction filters live here, and that is the point. This function had
/// a per-test-file copy in at least two behavioral tests whose only difference
/// was the filter set — one carried the `date:` rule, the other the
/// `Parsed using` rule. Same drift #1649 recorded for `redact_nondeterministic`,
/// one level up: the same output sanitized differently depending on which copy
/// recorded it, which makes a snapshot diff mean two different things.
/// Unifying here means a test cannot silently change what gets redacted by
/// moving.
pub(crate) fn assert_snapshot_redacted(name: &str, dir: &Path, value: impl Into<String>) {
    let redacted = redact_nondeterministic(dir, &value.into());
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"date: \d{4}-\d{2}-\d{2}", "date: [DATE]");
    settings.add_filter(r"(?m)^Parsed using .+$", "Parsed using [REDACTED]");
    settings.bind(|| {
        assert_snapshot!(name, redacted);
    });
}

pub(crate) fn assert_snapshot_plain(name: &str, value: impl Into<String>) {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"(?m)^Parsed using .+$", "Parsed using [REDACTED]");
    settings.bind(|| {
        assert_snapshot!(name, value.into());
    });
}

#[cfg(test)]
mod spawn_latency_budget {
    use super::{assert_spawn_within, run_under_budget, SpawnOutcome, SPAWN_LATENCY_BUDGET};
    use std::time::{Duration, Instant};

    /// The property #1697 needs and that a healthy runner does not show by
    /// itself: an overrunning attempt is ABANDONED at its budget, not allowed
    /// to run to completion. A closure sleeping 20x the budget must therefore
    /// be waited on for ~1x the budget per attempt, not ~20x. Without this the
    /// helper could silently degrade back into "wait for whatever the child
    /// does", which is exactly the 180 s burn it was written to stop.
    ///
    /// Asserted on the waits the harness ITSELF measured, never on this test's
    /// wall clock. The clock additionally pays for thread spawn, the panic
    /// hook, backtrace symbolization and unwinding; on `windows-latest` that
    /// overhead measured 398 ms against 211 ms of real waits, which tripped the
    /// old `elapsed < budget * 5` bound while EVERY wait was inside its budget
    /// (#2063). The waits are the property; the machinery is not.
    ///
    /// The 5x upper bound is deliberately loose — the counterexample is 20x —
    /// so a loaded runner's `recv_timeout` overshoot still fits. Worst observed
    /// overshoot is 247 ms against a 100 ms budget on `macos-latest`.
    #[test]
    fn an_overrunning_attempt_is_abandoned_at_its_budget() {
        let budget = Duration::from_millis(100);
        let sleep_ms = 20 * budget.as_millis() as u64;
        let outcome = run_under_budget(budget, move || {
            std::thread::sleep(Duration::from_millis(sleep_ms));
            Ok(())
        });
        let overruns = match outcome {
            SpawnOutcome::Overran(overruns) => overruns,
            other => {
                panic!("a closure outliving its budget must overrun both attempts, got {other:?}")
            },
        };
        assert_eq!(
            overruns.len(),
            2,
            "the helper retries once, so both attempts must be abandoned"
        );
        for latency in &overruns {
            assert!(
                *latency >= budget,
                "an abandonment cannot precede its budget, measured {latency:?}"
            );
            assert!(
                *latency < budget * 5,
                "the attempt sleeps {sleep_ms}ms, so an abandonment after \
                 {latency:?} means the budget is not bounding the wait"
            );
        }
    }

    /// The user-visible half of the same contract: an overrunning attempt must
    /// FAIL the caller with its budget and both measured latencies, not return.
    #[test]
    #[should_panic(expected = "stuck_spawn: exceeded its 100ms budget on 2 consecutive attempts")]
    fn issue_2063_an_overrunning_attempt_fails_the_test_naming_its_budget() {
        let budget = Duration::from_millis(100);
        let sleep_ms = 10 * budget.as_millis() as u64;
        assert_spawn_within(budget, "stuck_spawn", move || {
            std::thread::sleep(Duration::from_millis(sleep_ms));
            Ok(())
        });
    }

    /// The counterpart: a healthy attempt returns immediately and never pays
    /// the budget, so the helper adds no latency to the ~941 tests that spawn
    /// the binary normally.
    ///
    /// These three tests are compiled into every integration-test binary that
    /// includes `cli_harness.rs`, so this one ran in parallel with whatever
    /// else the runner was doing — and whichever of them warmed the binary
    /// first decided whether a cold `cargo build` landed inside this clock.
    /// Warming up front makes the pin independent of test order AND of which
    /// binary happens to run first.
    #[test]
    fn a_healthy_attempt_returns_without_paying_the_budget() {
        let _ = super::webfang_path();
        let started = Instant::now();
        assert_spawn_within(SPAWN_LATENCY_BUDGET, "healthy_spawn", || Ok(()));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "a passing attempt must not wait on the budget"
        );
    }

    /// A genuine failure must surface its own message, not be swallowed into
    /// the generic budget panic — otherwise the retry would hide the first
    /// verdict and report a timeout for a test that actually answered wrong.
    #[test]
    #[should_panic(expected = "expected code 64, got 0")]
    fn a_failing_attempt_reports_its_own_verdict() {
        assert_spawn_within(SPAWN_LATENCY_BUDGET, "wrong_code", || {
            Err("expected code 64, got 0".to_owned())
        });
    }

    /// The third way an attempt can end: the worker dies before sending, so its
    /// sender drops and no verdict can ever arrive. The caller must be told
    /// which attempt was lost instead of waiting out a budget for an answer
    /// that is already impossible — an independent verifier found this arm
    /// reachable but unpinned when the retry loop was extracted (#2063).
    #[test]
    #[should_panic(expected = "gone_spawn: the attempt ended without a verdict (attempt 1)")]
    fn issue_2063_a_disconnected_attempt_reports_the_attempt_number() {
        assert_spawn_within(SPAWN_LATENCY_BUDGET, "gone_spawn", || panic!("worker died"));
    }
}

#[cfg(test)]
mod redact_nondeterministic_divergence {
    use super::redact_nondeterministic;
    use std::path::Path;

    /// #1649: the redaction chain used to exist TWICE — here and in
    /// `webfang_test_utils` — and they drifted (only the local copy had the
    /// `#688` `<TRACE_ID>` rule), so the same output was sanitized
    /// differently depending on which harness recorded it. This file now
    /// re-exports the shared implementation, so the equality below is
    /// structural; the test is kept as the pin that says so, and it is the
    /// assertion that fails if anyone reintroduces a local copy.
    ///
    /// It lives here, in the file that used to be the divergent copy, so a
    /// reintroduced copy is caught at the exact place it would be written.
    #[test]
    fn local_harness_and_shared_utils_redact_identically() {
        let dir = Path::new("/tmp/.tmpABC123");
        let input = concat!(
            "wrote /tmp/.tmpABC123/output.md at 2024-03-15T10:30:00.123+01:00 ",
            "on 127.0.0.1:34567 WARN webfang_core::cli::orchestrator ",
            "trace_id=3f2504e0-4f89-11d3-9a0c-0305e82c3301",
        );

        let local = redact_nondeterministic(dir, input);
        let shared = webfang_test_utils::redact_nondeterministic(dir, input);

        assert_eq!(
            local, shared,
            "both paths must apply the same redaction chain (#1649)"
        );
        assert_eq!(
            local,
            "wrote <OUT_DIR>/output.md at <TIMESTAMP> on 127.0.0.1:<PORT> \
             WARN <MODULE> trace_id=<TRACE_ID>"
        );
    }
}
