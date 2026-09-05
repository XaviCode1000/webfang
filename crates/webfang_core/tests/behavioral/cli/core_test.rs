//! Core CLI behavior: version, help, missing/invalid URL.

use crate::cmd;
use predicates::prelude::*;

#[cfg(all(feature = "ai", feature = "adaptive-selectors"))]
use crate::assert_snapshot_plain;

// ---------------------------------------------------------------------------
// --version
// ---------------------------------------------------------------------------

#[test]
fn version_exits_zero() {
    cmd().arg("--version").assert().code(0);
}

#[test]
fn version_contains_version_string() {
    cmd()
        .arg("--version")
        .assert()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

// ---------------------------------------------------------------------------
// --help
// ---------------------------------------------------------------------------

#[test]
fn help_exits_zero() {
    cmd().arg("--help").assert().code(0);
}

#[test]
fn help_contains_url_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--url"));
}

#[test]
fn help_contains_single_page_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--single-page"));
}

#[test]
fn help_contains_format_flag() {
    // Slice 5b (#987) renamed `--format` to `--content-format`; the old
    // name is preserved as a hidden clap alias and so does not appear
    // in `--help` output. Test the canonical name (issue #980).
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--content-format"));
}

#[test]
fn help_contains_output_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--output"));
}

#[test]
fn help_contains_quiet_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--quiet"));
}

#[test]
fn help_contains_dry_run_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--dry-run"));
}

#[test]
fn help_contains_max_depth_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--max-depth"));
}

#[test]
fn help_contains_max_pages_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--max-pages"));
}

#[test]
fn help_contains_download_images_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--download-images"));
}

#[test]
fn help_contains_download_documents_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--download-documents"));
}

#[test]
fn help_contains_obsidian_wiki_links_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--obsidian-wiki-links"));
}

#[test]
fn help_contains_obsidian_tags_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--obsidian-tags"));
}

#[test]
fn help_contains_quick_save_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--quick-save"));
}

#[test]
fn help_contains_include_pattern_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--include-pattern"));
}

#[test]
fn help_contains_exclude_pattern_flag() {
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--exclude-pattern"));
}

// ---------------------------------------------------------------------------
// Missing --url
// ---------------------------------------------------------------------------

#[test]
fn no_url_exits_error() {
    cmd().assert().failure();
}

#[test]
fn no_url_stderr_mentions_url() {
    cmd().assert().stderr(predicate::str::contains("--url"));
}

#[test]
fn no_url_exit_code_64() {
    cmd().assert().code(64);
}

// ---------------------------------------------------------------------------
// Invalid URL
// ---------------------------------------------------------------------------

#[test]
fn invalid_url_exits_error() {
    cmd().arg("--url").arg("not-a-url").assert().failure();
}

#[test]
fn invalid_url_stderr_mentions_invalid() {
    cmd()
        .arg("--url")
        .arg("not-a-url")
        .assert()
        .stderr(predicate::str::contains("Invalid URL"));
}

#[test]
fn invalid_url_exit_code_64() {
    cmd().arg("--url").arg("not-a-url").assert().code(64);
}

// ---------------------------------------------------------------------------
// Removed flags (TUI removal)
// ---------------------------------------------------------------------------

/// The interactive TUI was deleted with its crate; `--tui` must fail at
/// clap parse time as an unexpected argument with the usage exit code 64
/// (`EXIT_USAGE_ERROR`), not panic — pinning the post-removal contract.
#[test]
fn removed_tui_flag_is_rejected_with_usage_error() {
    cmd()
        .arg("--tui")
        .assert()
        .code(64)
        .stderr(predicate::str::contains("unexpected argument '--tui'"));
}

// ---------------------------------------------------------------------------
// --help output snapshot (deterministic, network-free)
// Only runs when both the `ai` and `adaptive-selectors` features are enabled,
// because the snapshot includes AI-specific flags (--clean-ai, --threshold,
// --max-tokens, etc.; present only with `--features ai`) and the
// `--adaptive-selectors` flag (present only with `--features adaptive-selectors`).
// ---------------------------------------------------------------------------

#[cfg(all(feature = "ai", feature = "adaptive-selectors"))]
#[test]
fn help_output_snapshot() {
    let output = cmd().arg("--help").output().expect("run binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_snapshot_plain("help", stdout);
}
