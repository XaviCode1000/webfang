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

/// Resolve the path to the `webfang` binary.
///
/// `webfang` is built by the `webfang_cli` crate (a workspace sibling),
/// so `assert_cmd::cargo_bin` cannot locate it from `webfang_core` tests
/// — `CARGO_BIN_EXE_webfang` is only set for the crate that owns the binary.
/// We fall back to the workspace `target/` dir and, if missing, build it.
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
    for profile in ["debug", "release"] {
        let mut candidate = workspace_root.join("target").join(profile).join("webfang");
        if cfg!(windows) {
            candidate.set_extension("exe");
        }
        if candidate.exists() {
            return candidate;
        }
    }
    let cargo = option_env!("CARGO").unwrap_or("cargo");
    let status = std::process::Command::new(cargo)
        .args(["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"])
        .status()
        .expect("spawn cargo to build webfang");
    assert!(status.success(), "cargo build --bin webfang failed");
    let mut built = workspace_root.join("target").join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
}

/// Shared binary command builder for tests that don't need a mock server.
pub(crate) fn cmd() -> Command {
    Command::new(webfang_path())
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
        let mut cmd = Command::new(webfang_path());
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
        let mut cmd = Command::new(webfang_path());
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
        let mut cmd = Command::new(webfang_path());
        cmd.arg("--resume")
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
/// the temp dir, ISO-8601 log timestamps, dynamic wiremock ports, ANSI color
/// escape sequences, and environment-specific error suffixes (CI mode,
/// headless build notices) that differ between local and CI environments.
pub(crate) fn redact_nondeterministic(dir: &Path, text: &str) -> String {
    let text = redact_temp_path(dir, text);
    let ansi = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    let text = ansi.replace_all(&text, "").into_owned();
    // Normalize environment-specific error suffixes so snapshots are identical
    // across local and CI environments. The CLI appends "(CI mode)" when
    // is_ci() is true and "(interactive prompt requires --features ui)" in
    // headless builds — neither is deterministic across environments.
    let env_suffix =
        Regex::new(r" \(CI mode\)| \(interactive prompt requires --features ui\)").unwrap();
    let text = env_suffix.replace_all(&text, "").into_owned();
    let ts =
        Regex::new(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?([+-]\d{2}:?\d{2}|Z)").unwrap();
    let text = ts.replace_all(&text, "<TIMESTAMP>").into_owned();
    let port = Regex::new(r"127\.0\.0\.1:\d+").unwrap();
    port.replace_all(&text, "127.0.0.1:<PORT>").into_owned()
}
