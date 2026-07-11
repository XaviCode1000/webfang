//! Core CLI behavior: version, help, missing/invalid URL.

use crate::cmd;
use predicates::prelude::*;

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
    cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--format"));
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
