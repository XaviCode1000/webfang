//! B37 — comments and string literals cannot hide a site, and cannot invent one.
//!
//! `syn` drops comments and turns strings into literal values, so a path inside
//! a string is invisible and a `/*` inside a string cannot blind the file.

#[path = "common/mod.rs"]
mod common;

use common::TempTree;

#[test]
fn a_string_cannot_blind_the_file_and_is_not_reported() {
    let tree = TempTree::new("b37");
    tree.write(
        "b37.rs",
        "const BLOCK: &str = \"/* not a comment\";\n\
         const PATH: &str = \"crate::infrastructure::InString\";\n\
         // crate::infrastructure::InLineComment\n\
         /* crate::infrastructure::InBlockComment */\n\
         fn production() -> crate::domain::Production {\n\
         \x20   crate::domain::Production\n\
         }\n",
    );

    assert_eq!(
        tree.records(),
        vec![
            "5\tinline\tcrate::domain::Production",
            "6\tinline\tcrate::domain::Production",
        ]
    );
}

#[test]
fn a_trailing_line_comment_is_not_reported() {
    let tree = TempTree::new("b37-trailing");
    tree.write(
        "trailing.rs",
        "fn production() -> crate::domain::Production {\n\
         \x20   let _ = 1; // crate::infrastructure::InComment\n\
         \x20   crate::domain::Production\n\
         }\n",
    );

    assert_eq!(
        tree.records(),
        vec![
            "1\tinline\tcrate::domain::Production",
            "3\tinline\tcrate::domain::Production",
        ]
    );
}
