//! B38 — every path form is reported, normalised.
//!
//! Use trees, renamed imports, globs, nested groups and inline qualified paths
//! are all extracted; the alias of a renamed import is not part of the path, and
//! a leading `::` is dropped.

#[path = "common/mod.rs"]
mod common;

use common::TempTree;

const FIXTURE: &str = r#"use crate::infrastructure::{A, B};
use crate::infrastructure;
use crate::infrastructure::*;
use crate::{infrastructure::Foo};
use crate::infrastructure as infra;
fn f() -> crate::domain::Thing { crate :: infrastructure :: Foo }
fn g() -> ::crate::domain::Thing { ::crate::domain::Thing }
fn h() { self::infrastructure::x(); }
"#;

#[test]
fn every_path_form_is_reported_with_the_expected_record() {
    let tree = TempTree::new("b38");
    tree.write("b38.rs", FIXTURE);

    assert_eq!(
        tree.records(),
        vec![
            "1\tuse\tcrate::infrastructure::A",
            "1\tuse\tcrate::infrastructure::B",
            "2\tuse\tcrate::infrastructure",
            "3\tuse\tcrate::infrastructure",
            "4\tuse\tcrate::infrastructure::Foo",
            "5\tuse\tcrate::infrastructure",
            "6\tinline\tcrate::domain::Thing",
            "6\tinline\tcrate::infrastructure::Foo",
            "7\tinline\tcrate::domain::Thing",
        ]
    );
}

#[test]
fn a_self_prefix_is_not_a_site() {
    let tree = TempTree::new("b38-self");
    tree.write("self.rs", "fn h() { self::infrastructure::x(); }\n");

    assert!(tree.records().is_empty());
}

#[test]
fn a_leading_colon_is_dropped() {
    let tree = TempTree::new("b38-colon");
    tree.write(
        "colon.rs",
        "fn g() -> ::crate::domain::Thing { ::crate::domain::Thing }\n",
    );

    // The return type and the body expression are the same path on the same
    // line, so they collapse into one record — the same key the gate dedupes on.
    assert_eq!(tree.records(), vec!["1\tinline\tcrate::domain::Thing"]);
}

#[test]
fn every_leaf_of_a_use_statement_lands_on_the_statement_line() {
    let tree = TempTree::new("b38-multiline");
    tree.write(
        "multiline.rs",
        "use crate::infrastructure::crawler::{\n\
         \x20   SitemapConfig,\n\
         \x20   SitemapError,\n\
         };\n",
    );

    assert_eq!(
        tree.records(),
        vec![
            "1\tuse\tcrate::infrastructure::crawler::SitemapConfig",
            "1\tuse\tcrate::infrastructure::crawler::SitemapError",
        ]
    );
}
