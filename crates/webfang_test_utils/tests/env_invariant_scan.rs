//! Source-scan enforcement of the workspace ENV_LOCK invariant (#1126,
//! #1349).
//!
//! Mechanical gate: no `.rs` file outside the sanctioned owner
//! (`webfang_test_utils`) may contain a raw `env::set_var` /
//! `env::remove_var` call. Complements the clippy `disallowed-methods`
//! deny (active in the webfang_core and webfang_mcp src trees) by ALSO
//! walking integration-test trees, which are separate crates and do not
//! inherit a lib.rs deny attribute. Std-only and deterministic on
//! purpose: this must run anywhere, with no dependencies of its own.

use std::path::{Path, PathBuf};

/// Workspace root resolved from this crate's manifest directory:
/// `CARGO_MANIFEST_DIR` is `crates/webfang_test_utils`, so one hop up is
/// `crates` and two hops up is the workspace root.
fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("resolve workspace root from CARGO_MANIFEST_DIR")
}

/// Recursively collect `.rs` files under `dir` into `out`.
fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // A missing tree (e.g. a crate without a tests/ directory) has
        // nothing to scan; that is not a violation.
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// A reintroduced raw `std::env::set_var` / `std::env::remove_var` outside
/// the sanctioned owner must fail this test.
///
/// Walks every `.rs` file under `crates/*/src/` (recursively, which includes
/// `src/bin`) and `crates/*/tests/`, excluding the whole
/// `crates/webfang_test_utils/` directory (the ENV_LOCK owner) and this scan
/// file itself. The needles are built with `concat!` so the scanner never
/// self-matches its own source.
#[test]
fn env_reader_without_guard_fails_fast() {
    let needle_set = concat!("env::", "set_var");
    let needle_rm = concat!("env::", "remove_var");
    let root = workspace_root();
    let crates_dir = root.join("crates");
    let excluded_owner = crates_dir.join("webfang_test_utils");
    let scan_file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("env_invariant_scan.rs");

    let mut files: Vec<PathBuf> = Vec::new();
    let crate_entries = std::fs::read_dir(&crates_dir)
        .expect("workspace crates/ directory must exist and be readable");
    for entry in crate_entries.flatten() {
        let crate_dir = entry.path();
        if !crate_dir.is_dir() || crate_dir == excluded_owner {
            continue;
        }
        collect_rs_files(&crate_dir.join("src"), &mut files);
        collect_rs_files(&crate_dir.join("tests"), &mut files);
    }
    // Deterministic violation order regardless of directory iteration order.
    files.sort();

    let mut violations: Vec<String> = Vec::new();
    for file in &files {
        if *file == scan_file {
            continue;
        }
        let bytes = match std::fs::read(file) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        let source = String::from_utf8_lossy(&bytes);
        for (idx, line) in source.lines().enumerate() {
            if line.contains(needle_set) || line.contains(needle_rm) {
                let rel = file
                    .strip_prefix(&root)
                    .unwrap_or(file)
                    .display()
                    .to_string();
                violations.push(format!("  {rel}:{}: {}", idx + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "raw process-env mutations outside webfang_test_utils (the ENV_LOCK \
         owner) are forbidden (#1126, #1349) — use webfang_test_utils \
         (EnvGuard::with/clean/set/remove, env_set/env_remove, or env_lock); \
         offenders:\n{}",
        violations.join("\n")
    );
}
