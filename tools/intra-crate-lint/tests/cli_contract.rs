//! The CLI contract: stdout carries the records, and the exit code carries the
//! outcome — 0 ok, 1 usage error, 2 a file that could not be read or parsed.

#[path = "common/mod.rs"]
mod common;

use std::process::Command;

use common::TempTree;

fn run(arguments: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_intra-crate-lint"))
        .args(arguments)
        .output()
        .expect("run the extractor");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn a_clean_tree_exits_zero_and_prints_the_records() {
    let tree = TempTree::new("cli-ok");
    tree.write("good.rs", "use crate::domain::Thing;\n");

    let (code, stdout, stderr) = run(&["--root", tree.root.to_str().expect("utf-8 path")]);

    assert_eq!(code, 0);
    assert_eq!(stdout, "good.rs\t1\tuse\tcrate::domain::Thing\n");
    assert!(stderr.is_empty(), "unexpected stderr: {stderr}");
}

#[test]
fn a_parse_error_exits_two_and_names_the_file() {
    let tree = TempTree::new("cli-broken");
    tree.write("good.rs", "use crate::domain::Thing;\n");
    tree.write("broken.rs", "fn broken( {\n");

    let (code, stdout, stderr) = run(&["--root", tree.root.to_str().expect("utf-8 path")]);

    assert_eq!(code, 2, "stderr: {stderr}");
    assert!(
        stderr.contains("broken.rs"),
        "stderr must name the file: {stderr}"
    );
    assert!(
        !stderr.contains("good.rs"),
        "stderr must not name the good file: {stderr}"
    );
    assert!(
        stdout.contains("good.rs\t1\tuse\tcrate::domain::Thing"),
        "the readable file is still reported: {stdout}"
    );
}

#[test]
fn a_missing_root_is_a_usage_error() {
    let (code, _stdout, stderr) = run(&[]);

    assert_eq!(code, 1);
    assert!(stderr.contains("--root"), "stderr: {stderr}");
}

#[test]
fn an_unknown_argument_is_a_usage_error() {
    let (code, _stdout, stderr) = run(&["--nope"]);

    assert_eq!(code, 1);
    assert!(stderr.contains("--nope"), "stderr: {stderr}");
}

#[test]
fn an_unreadable_root_is_reported_on_stderr() {
    let (code, _stdout, stderr) = run(&["--root", "/nonexistent/intra-crate-lint-root"]);

    assert_eq!(code, 2);
    assert!(stderr.contains("intra-crate-lint-root"), "stderr: {stderr}");
}
