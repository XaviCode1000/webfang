//! The differential measurement against the real tree.
//!
//! The golden set is the extractor's complete output over
//! `crates/webfang_core/src` at the moment it was measured. It pins the
//! extractor's behaviour on the real code.
//!
//! The golden set is only meaningful for the tree it was measured on: a change
//! to `crates/webfang_core/src` legitimately changes it, and the set is
//! refreshed in the same commit that changes the tree.
//!
//! The property that makes the extractor worth its cost — it sees the sites
//! the old awk scanner could not — is proved by a SYNTHETIC tree below, not by
//! a real violation in the repository. It used to be proved by a live site
//! (`domain/ssrf_guard.rs:135`, a `domain` module importing `application`,
//! which the awk scanner missed because a doc comment mentioning
//! `#[cfg(test)]` blinded the rest of that file). That site was FIXED — the
//! constant it named moved to the domain — and a witness that stops existing
//! when the defect is closed is a witness that rots: it would fail the moment
//! the repository is repaired. The property is a property of the extractor, so
//! it is now asserted on a fixture.

#[path = "common/mod.rs"]
mod common;

use std::fs;
use std::path::PathBuf;

use common::{real_tree_root, TempTree};
use intra_crate_lint::extract_tree;


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

/// A module whose DOC COMMENT mentions `#[cfg(test)]` before the import.
///
/// The mention is prose, not an attribute. A scanner that classifies by the
/// text of the line mistakes the rest of the file for a test region and
/// reports nothing from it — the blind spot that let a `domain` module import
/// `application` for months without the gate seeing it.
const BLIND_SPOT_FILE: &str = r#"//! A module whose doc comment mentions `#[cfg(test)]`.
//!
//! The mention is prose, not an attribute: a scanner that classifies by the
//! text of the line mistakes the rest of this file for a test region.

/// The read sits behind `#[cfg(test)]` in production builds.
use crate::application::llm_extraction::LLM_SSRF_TEST_HATCH_ENV;
"#;

#[test]
fn the_extractor_sees_past_the_doc_comment_blind_spot() {
    let tree = TempTree::new("blindspot");
    tree.write("domain/ssrf_guard.rs", BLIND_SPOT_FILE);

    let records = tree.records();
    assert!(
        records
            .iter()
            .any(|record| record.ends_with("\tuse\tcrate::application::llm_extraction::LLM_SSRF_TEST_HATCH_ENV")),
        "the extractor must see the import that follows a doc comment mentioning #[cfg(test)]: {records:?}"
    );
}
