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

/// Resolve the path to the `webfang` binary.
///
/// `webfang` is built by the `rust_scraper_cli` crate (a workspace sibling),
/// so `assert_cmd::cargo_bin` cannot locate it from `rust_scraper_core` tests
/// — `CARGO_BIN_EXE_webfang` is only set for the crate that owns the binary.
/// We fall back to the workspace `target/` dir and, if missing, build it.
pub(crate) fn webfang_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return std::path::PathBuf::from(p);
    }
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/rust_scraper_core -> workspace root (two levels up)
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
        .args([
            "build",
            "-p",
            "rust_scraper_cli",
            "--bin",
            "webfang",
            "--quiet",
        ])
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
