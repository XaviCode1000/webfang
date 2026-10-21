//! Shared helpers for the extractor's integration tests.

// Each test target compiles this module on its own and uses a different subset
// of it, so the unused helpers are expected.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use intra_crate_lint::{extract_tree, ExtractError, Site};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A throwaway directory tree that is removed when the test ends.
pub struct TempTree {
    /// The root of the tree.
    pub root: PathBuf,
}

impl TempTree {
    /// Create an empty tree tagged with `tag`.
    pub fn new(tag: &str) -> TempTree {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "intra-crate-lint-{tag}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create the temporary tree");
        TempTree { root }
    }

    /// Write `contents` to `relative` inside the tree, creating parents.
    pub fn write(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create the parent directory");
        }
        fs::write(&path, contents).expect("write the fixture file");
    }

    /// Every site in the tree; panics when a file failed to parse.
    pub fn sites(&self) -> Vec<Site> {
        let extraction = extract_tree(&self.root).expect("scan the temporary tree");
        assert!(
            extraction.errors.is_empty(),
            "unexpected extraction errors: {:?}",
            extraction.errors
        );
        extraction.sites
    }

    /// Every site in the tree, rendered as `<line>\t<kind>\t<path>`.
    pub fn records(&self) -> Vec<String> {
        self.sites()
            .iter()
            .map(|site| format!("{}\t{}\t{}", site.line, site.kind, site.path))
            .collect()
    }

    /// The errors of a scan that is expected to fail.
    pub fn errors(&self) -> Vec<ExtractError> {
        let extraction = extract_tree(&self.root).expect("scan the temporary tree");
        extraction.errors
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The repository's `crates/webfang_core/src` tree, resolved from the tool's
/// manifest directory so the test does not depend on the working directory.
pub fn real_tree_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("..")
        .join("..")
        .join("crates")
        .join("webfang_core")
        .join("src")
}
