//! Headless TUI fallback tests (spec S2.2).
//!
//! When the `ui` feature is OFF, the `--tui`, `--config-tui`, and `--interactive`
//! flags MUST print a Spanish message and exit gracefully instead of attempting
//! to render a TUI. These tests run ONLY when `ui` is not enabled, proving the
//! core binary works headless without ratatui/crossterm.

#![cfg(not(feature = "ui"))]

use assert_cmd::Command;

/// Expected Spanish message (spec S2.2 exact wording).
const EXPECTED_MSG: &str = "TUI no disponible: compilar con --features ui";

fn webfang_core() -> Command {
    Command::cargo_bin("webfang_core")
        .expect("webfang_core binary must be built for this test")
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

#[test]
fn config_tui_flag_prints_spanish_message_when_ui_off() {
    let output = webfang_core()
        .arg("--config-tui")
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .expect("failed to execute webfang_core");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !output.status.success(),
        "--config-tui must exit non-zero when ui is OFF; got exit {:?}",
        output.status.code()
    );
    assert!(
        combined.contains(EXPECTED_MSG),
        "--config-tui must print the Spanish TUI-unavailable message\nexpected: {EXPECTED_MSG}\nstderr: {stderr}"
    );
}

#[test]
fn interactive_flag_prints_spanish_message_when_ui_off() {
    let output = webfang_core()
        .arg("--interactive")
        .timeout(std::time::Duration::from_secs(10))
        .output()
        .expect("failed to execute webfang_core");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout}{stderr}");

    assert!(
        !output.status.success(),
        "--interactive must exit non-zero when ui is OFF; got exit {:?}",
        output.status.code()
    );
    assert!(
        combined.contains(EXPECTED_MSG),
        "--interactive must print the Spanish TUI-unavailable message\nexpected: {EXPECTED_MSG}\nstderr: {stderr}"
    );
}
