//! Headless TUI fallback tests (spec S2.2).
//!
//! When the `ui` feature is OFF, the `--tui` flag MUST print a Spanish message
//! and exit gracefully instead of attempting to render a TUI. These tests run
//! ONLY when `ui` is not enabled, proving the core binary works headless
//! without ratatui/crossterm.

#![cfg(not(feature = "ui"))]

#[path = "common/cli_harness.rs"]
mod common;

use assert_cmd::Command;
use common::webfang_path;

/// Expected Spanish message (spec S2.2 exact wording).
const EXPECTED_MSG: &str = "interfaz TUI no está disponible";

fn webfang_core() -> Command {
    Command::new(webfang_path())
}

#[test]
fn tui_flag_prints_spanish_message_when_ui_off() {
    let output = webfang_core()
        .arg("--tui")
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .expect("failed to execute webfang_core");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !output.status.success(),
        "--tui must exit non-zero when ui is OFF; got exit {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status.code()
    );
    assert!(
        combined.contains(EXPECTED_MSG),
        "--tui must print the Spanish TUI-unavailable message\nexpected substring: {EXPECTED_MSG}\nstdout: {stdout}\nstderr: {stderr}"
    );
}
