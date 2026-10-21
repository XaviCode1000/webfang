//! B36 — test regions are recognised structurally, over `syn::Meta`.
//!
//! A node is inside a test region when it carries a `#[cfg(P)]` whose predicate
//! positively mentions `test`. The exclusion is the test-module SUBTREE, so
//! code that follows a `#[cfg(test)] mod` in the same file is still scanned.

#[path = "common/mod.rs"]
mod common;

use common::TempTree;

const FIXTURE: &str = r#"/// A doc comment that mentions `#[cfg(test)]` does not open a test region.
///
/// The path `crate::infrastructure::DocOnly` lives inside a doc comment.
use crate::domain::TopLevel;

#[cfg(test)]
mod tests {
    use crate::infrastructure::InTestModule;

    #[test]
    fn inside_test_module() {
        let _ = crate::application::InsideTestFunction;
    }
}

#[cfg(all(test, not(miri)))]
mod all_test {
    use crate::infrastructure::InAllTest;
}

#[cfg(any(test, feature = "extra"))]
mod any_test {
    use crate::infrastructure::InAnyTest;
}

#[cfg(not(test))]
mod not_test {
    use crate::infrastructure::InNotTest;
}

#[cfg_attr(test, allow(dead_code))]
mod cfg_attr_test {
    use crate::infrastructure::InCfgAttr;
}

use crate::infrastructure::AfterTestModule;

fn production() -> crate::domain::Production {
    crate::domain::Production
}
"#;

#[test]
fn test_regions_are_excluded_and_production_code_is_not() {
    let tree = TempTree::new("b36");
    tree.write("b36.rs", FIXTURE);

    let records = tree.records();
    assert_eq!(
        records,
        vec![
            "4\tuse\tcrate::domain::TopLevel",
            "28\tuse\tcrate::infrastructure::InNotTest",
            "33\tuse\tcrate::infrastructure::InCfgAttr",
            "36\tuse\tcrate::infrastructure::AfterTestModule",
            "38\tinline\tcrate::domain::Production",
            "39\tinline\tcrate::domain::Production",
        ]
    );
}

#[test]
fn a_doc_comment_mentioning_cfg_test_is_not_a_test_region() {
    let tree = TempTree::new("b36-doc");
    tree.write(
        "doc.rs",
        "/// The read sits behind `#[cfg(test)]` in the guard.\n\
         use crate::application::AfterDocComment;\n",
    );

    assert_eq!(
        tree.records(),
        vec!["2\tuse\tcrate::application::AfterDocComment"]
    );
}

#[test]
fn code_after_a_test_module_is_scanned() {
    let tree = TempTree::new("b36-after");
    tree.write(
        "after.rs",
        "#[cfg(test)]\n\
         mod tests {\n\
         \x20   use crate::infrastructure::Hidden;\n\
         }\n\
         \n\
         use crate::infrastructure::Scanned;\n",
    );

    assert_eq!(
        tree.records(),
        vec!["6\tuse\tcrate::infrastructure::Scanned"]
    );
}

#[test]
fn a_test_only_method_inside_an_impl_block_is_excluded() {
    let tree = TempTree::new("b36-impl");
    tree.write(
        "impl.rs",
        "struct Queue;\n\
         \n\
         impl Queue {\n\
         \x20   fn production(&self) -> crate::domain::Thing {\n\
         \x20       crate::domain::Thing\n\
         \x20   }\n\
         \n\
         \x20   #[cfg(test)]\n\
         \x20   fn test_only(&self) -> crate::infrastructure::Helper {\n\
         \x20       crate::infrastructure::Helper\n\
         \x20   }\n\
         }\n",
    );

    assert_eq!(
        tree.records(),
        vec![
            "4\tinline\tcrate::domain::Thing",
            "5\tinline\tcrate::domain::Thing"
        ]
    );
}
