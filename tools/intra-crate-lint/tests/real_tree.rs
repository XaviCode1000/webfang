//! The differential measurement against the real tree.
//!
//! The golden set is the extractor's complete output over
//! `crates/webfang_core/src` at the moment the tool was written. It pins the
//! extractor's behaviour on the real code, and it carries the one site the
//! current awk gate cannot see: `domain/ssrf_guard.rs:135` — a `domain` module
//! importing `application`, which is OUTWARD, and which the awk gate misses
//! because a doc comment mentioning `#[cfg(test)]` at line 125 blinds the rest
//! of that file.
//!
//! The golden set is only meaningful for the tree it was measured on: a change
//! to `crates/webfang_core/src` legitimately changes it.

#[path = "common/mod.rs"]
mod common;

use std::fs;
use std::path::PathBuf;

use common::real_tree_root;
use intra_crate_lint::extract_tree;

/// The site the awk gate cannot see: `domain` importing `application`.
const HIDDEN_VIOLATION: &str =
    "domain/ssrf_guard.rs\t135\tuse\tcrate::application::llm_extraction::LLM_SSRF_TEST_HATCH_ENV\n";

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("real_tree_sites.txt")
}

#[test]
fn the_real_tree_parses_without_errors_and_matches_the_golden_set() {
    let root = real_tree_root();
    assert!(
        root.is_dir(),
        "the real tree is missing: {}",
        root.display()
    );

    let extraction = extract_tree(&root).expect("scan the real tree");
    assert!(
        extraction.errors.is_empty(),
        "the real tree must parse without errors: {:?}",
        extraction.errors
    );

    let produced: String = extraction
        .sites
        .iter()
        .map(|site| format!("{site}\n"))
        .collect();
    let golden = fs::read_to_string(golden_path()).expect("read the golden set");

    assert_eq!(
        produced.lines().count(),
        golden.lines().count(),
        "the number of sites changed"
    );
    assert_eq!(produced, golden, "the extracted sites changed");
}

#[test]
fn the_hidden_violation_is_part_of_the_golden_set() {
    let golden = fs::read_to_string(golden_path()).expect("read the golden set");

    assert!(
        golden.contains(HIDDEN_VIOLATION),
        "the golden set must carry the site the awk gate cannot see"
    );
}
