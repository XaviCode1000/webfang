//! CLI for the intra-crate layer reference extractor.
//!
//! ```text
//! intra-crate-lint --root <dir>
//! ```
//!
//! * stdout: one TSV record per site — `<relative-file>\t<line>\t<kind>\t<path>`
//!   with `kind` in {`use`, `inline`, `choke`}.
//! * exit 0: the tree was scanned completely.
//! * exit 1: usage error (unknown argument, missing `--root`).
//! * exit 2: a file could not be read or parsed. Every failure is named on
//!   stderr — a parse error is never a silently skipped file.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use intra_crate_lint::extract_tree;

const USAGE: &str = "usage: intra-crate-lint --root <dir>";

fn main() -> ExitCode {
    match parse_args(std::env::args().skip(1)) {
        Err(message) => {
            eprintln!("intra-crate-lint: {message}");
            eprintln!("{USAGE}");
            ExitCode::from(1)
        },
        Ok(None) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        },
        Ok(Some(root)) => extract(&root),
    }
}

/// Scan the tree and report the outcome through the exit code.
fn extract(root: &Path) -> ExitCode {
    let extraction = match extract_tree(root) {
        Ok(extraction) => extraction,
        Err(error) => {
            eprintln!("intra-crate-lint: {error}");
            return ExitCode::from(2);
        },
    };
    for site in &extraction.sites {
        println!("{site}");
    }
    for error in &extraction.errors {
        eprintln!("intra-crate-lint: {error}");
    }
    if extraction.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

/// Parse the command line, returning the root directory.
///
/// `Ok(None)` means the user asked for help.
fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<PathBuf>, String> {
    let mut root: Option<PathBuf> = None;
    let mut args = args.peekable();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => match args.next() {
                Some(value) => root = Some(PathBuf::from(value)),
                None => return Err("--root requires a directory".to_string()),
            },
            "--help" | "-h" => return Ok(None),
            other if other.starts_with("--root=") => {
                root = Some(PathBuf::from(&other["--root=".len()..]));
            },
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    match root {
        Some(root) => Ok(Some(root)),
        None => Err("--root <dir> is required".to_string()),
    }
}
