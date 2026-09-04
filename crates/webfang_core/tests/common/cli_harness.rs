//! Shared CLI behavioral test harness for the `webfang` binary.
//!
//! `#![allow(dead_code)]`: this file is included via `#[path]` from three
//! separate test crates (`behavioral`, `cli_binary`, `cli_behavioral`), each of
//! which uses a different subset of the helpers. Items unused by a given crate
//! would otherwise trip `-D warnings`; gating them here is intentional.
#![allow(dead_code)]
//!
//! Centralized helpers used by the `behavioral`, `cli_binary`, and
//! `cli_behavioral` test binaries so the `webfang_path()` resolver, the
//! `BehavioralTest` mock-server/temp-dir harness, and the output-redaction
//! helpers live in exactly one place.
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
use regex::Regex;
use std::path::Path;
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, ResponseTemplate};

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
    let mut active_features = Vec::new();
    if cfg!(feature = "ai") {
        active_features.push("ai");
    }
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
    if cfg!(feature = "images") {
        active_features.push("images");
    }
    if cfg!(feature = "documents") {
        active_features.push("documents");
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

/// Remove all `WEBFANG_*`, `WEBFANG_AI_MODEL_ID`, and `AI_MODEL_ID` env
/// vars from a command so tests are hermetic even when CI bug-discovery
/// workflows poison the environment, and point `XDG_CACHE_HOME` at a fresh
/// per-invocation temp directory. The legacy `AI_MODEL_ID` is still
/// honored by `webfang_ai::infrastructure_ai::compat::read_ai_model_id()`
/// (the default-accesor wrapper over `read_ai_model_id_with`)
/// (#980 slice 5b), so a poisoned run exercises that fallback.
///
/// Hermetic cache: since the PersistenceMode wiring made checkpointing
/// default-on, every ordinary scrape loads and saves
/// `<cache>/webfang/state/crawl_checkpoint.json` from the default cache.
/// Parallel behavioral tests all run against `127.0.0.1` wiremock servers
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
    let poisoned: Vec<String> = std::env::vars()
        .filter(|(k, _)| k.starts_with("WEBFANG_") || k == "AI_MODEL_ID")
        .map(|(k, _)| k)
        .collect();
    for key in poisoned {
        cmd.env_remove(&key);
    }
    cmd.env("XDG_CACHE_HOME", hermetic_cache_dir());
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

/// Redact the per-run temp-dir path so snapshots stay stable across machines.
///
/// Output paths embed an absolute `TempDir` location that changes on every
/// run; collapse it to the fixed placeholder `<OUT_DIR>` before snapshotting.
pub(crate) fn redact_temp_path(dir: &Path, text: &str) -> String {
    text.replace(dir.to_string_lossy().as_ref(), "<OUT_DIR>")
}

/// Redact common non-deterministic output so snapshots are stable run-to-run:
/// the temp dir, ISO-8601 log timestamps, dynamic wiremock ports, and ANSI
/// color escape sequences.
pub(crate) fn redact_nondeterministic(dir: &Path, text: &str) -> String {
    let text = redact_temp_path(dir, text);
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let text = ansi.replace_all(&text, "").into_owned();
    let ts =
        Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)").unwrap();
    let text = ts.replace_all(&text, "<TIMESTAMP>").into_owned();
    let port = Regex::new(r"127\.0\.0\.1:\d+").unwrap();
    let text = port.replace_all(&text, "127.0.0.1:<PORT>").into_owned();
    // Normalize source line numbers in tracing spans (e.g. "scrape_flow.rs:193").
    // These shift with #[cfg(feature = "...")] blocks and differ across feature sets.
    let line_no = Regex::new(r"(\.rs:)\d+").unwrap();
    let text = line_no.replace_all(&text, "$1<LINE>").into_owned();
    // Normalize tracing module paths (e.g. "WARN webfang_core::cli::orchestrator:")
    // so snapshots decouple from source location and survive function moves (#462).
    let module = Regex::new(r"((?:WARN|INFO|ERROR|DEBUG|TRACE)\s+)\w+(?:::\w+)+").unwrap();
    let text = module.replace_all(&text, "$1<MODULE>").into_owned();
    // Normalize trace/correlation UUIDs emitted by log_scrape_error's trace_id
    // field (#688) so trace snapshots stay deterministic run-to-run.
    let trace_id =
        Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
            .unwrap();
    let text = trace_id.replace_all(&text, "<TRACE_ID>").into_owned();
    // Normalize tracing source file paths (e.g. "at crates/.../orchestrator.rs:<LINE>")
    // so moving a function between files does not break snapshots (#462).
    let file_path = Regex::new(r"(at\s+)\S+\.rs").unwrap();
    file_path.replace_all(&text, "$1<FILE>.rs").into_owned()
}
