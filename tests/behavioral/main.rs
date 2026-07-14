//! Adapter-layer behavioral tests for the `webfang` binary.
//!
//! Every test uses wiremock (no real network) and TempDir (auto-cleanup).
//! Run with: `cargo nextest run --test behavioral`

mod cli;

use assert_cmd::Command;

/// Returns the binary name to test, based on active features.
/// The full `webfang` binary requires both `ai` and `mcp`; the
/// `rust_scraper_core` binary is always built (default features).
fn cli_bin() -> &'static str {
    if cfg!(all(feature = "ai", feature = "mcp")) {
        "webfang"
    } else {
        "rust_scraper_core"
    }
}

/// Shared binary command builder for tests that don't need a mock server.
pub(crate) fn cmd() -> Command {
    Command::cargo_bin(cli_bin()).expect("binary exists")
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
        let mut cmd = assert_cmd::Command::cargo_bin(cli_bin()).expect("binary exists");
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
