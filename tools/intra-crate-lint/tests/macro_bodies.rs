//! Macro bodies are not parsed by `syn`, so their token streams are walked
//! directly: an identifier `crate` followed by `::` and another identifier
//! starts a path that is reconstructed from the tokens.

#[path = "common/mod.rs"]
mod common;

use common::TempTree;

#[test]
fn a_crate_path_inside_a_macro_body_is_reported() {
    let tree = TempTree::new("macro");
    tree.write(
        "macro.rs",
        "fn f() {\n\
         \x20   some_macro!(crate::infrastructure::in_macro);\n\
         \x20   other_macro!(crate::application::x, 1);\n\
         }\n",
    );

    assert_eq!(
        tree.records(),
        vec![
            "2\tinline\tcrate::infrastructure::in_macro",
            "3\tinline\tcrate::application::x",
        ]
    );
}

#[test]
fn a_crate_path_inside_a_nested_macro_group_is_reported() {
    let tree = TempTree::new("macro-nested");
    tree.write(
        "nested.rs",
        "fn f() {\n\
         \x20   some_macro!({ crate::infrastructure::in_group });\n\
         }\n",
    );

    assert_eq!(
        tree.records(),
        vec!["2\tinline\tcrate::infrastructure::in_group"]
    );
}

#[test]
fn a_macro_without_a_crate_path_reports_nothing() {
    let tree = TempTree::new("macro-empty");
    tree.write(
        "empty.rs",
        "fn f() {\n    some_macro!(alpha::beta, 1);\n}\n",
    );

    assert!(tree.records().is_empty());
}
