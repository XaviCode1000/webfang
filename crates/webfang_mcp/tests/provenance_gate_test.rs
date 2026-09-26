//! Mechanical provenance gate (audit M1, PI-3 invariant; issue #1600).
//!
//! This is a STRUCTURAL gate in the same spirit as
//! `scripts/check_dependency_direction.sh` (#513): it parses the source tree
//! at test time (via `CARGO_MANIFEST_DIR`) and fails when the invariant
//! "the only sanctioned `Content::text` constructors live in
//! `mcp_server::provenance`, and every tool description carries the standard
//! injection notice" regresses. It does NOT reason about runtime behavior —
//! it just makes the provenance invariant mechanical.
//!
//! - Gate 1: no file under `src/mcp_server/handlers/` may contain the literal
//!   `CallToolResult::success(vec![Content::text(` / `CallToolResult::error(`
//!   `vec![Content::text(` — and, stronger, no bare `Content::text(` at all.
//! - Gate 2: every `#[tool(` registration's description block must contain
//!   the standard notice sentence exported as
//!   `webfang_mcp::mcp_server::provenance::INJECTION_NOTICE`. Counting is
//!   deliberately dumb: each `#[tool(` owns everything up to the next
//!   `#[tool(` (or end of file); that region must contain the notice.

use std::path::{Path, PathBuf};

const HANDLERS_DIR: &str = "src/mcp_server/handlers";

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_source(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("gate must read {}: {e}", path.display()))
}

/// Collect every `#[tool(` chunk of a source file: each chunk spans from the
/// attribute up to (not including) the next `#[tool(` or EOF.
fn tool_chunks(source: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    while let Some(i) = source[offset..].find("#[tool(") {
        let start = offset + i;
        let next = source[start + 1..]
            .find("#[tool(")
            .map(|j| start + 1 + j)
            .unwrap_or(source.len());
        chunks.push(&source[start..next]);
        offset = next;
    }
    chunks
}

/// Gate 1: handlers never construct tool content directly.
#[test]
fn handlers_never_construct_tool_content_directly() {
    let dir = manifest_dir().join(HANDLERS_DIR);
    let mut checked = 0usize;
    for entry in std::fs::read_dir(&dir).expect("handlers dir must exist") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        checked += 1;
        let source = read_source(&path);
        for forbidden in [
            "CallToolResult::success(vec![Content::text(",
            "CallToolResult::error(vec![Content::text(",
            "CallToolResult::error(vec![Content::text(",
            "CallToolResult::success(vec![Content::text(",
            "Content::text(",
        ] {
            assert!(
                !source.contains(forbidden),
                "PROVENANCE GATE: {} contains the forbidden literal {forbidden:?}. \
                 Route tool results through `crate::mcp_server::provenance` \
                 (untrusted_text / local_text / neutralized_error) — see \
                 docs/security/prompt-injection-policy.md and issue #1600.",
                path.display()
            );
        }
    }
    assert!(
        checked >= 9,
        "expected the 9 handler modules, got {checked}"
    );
}

/// Gate 2: every `#[tool(` registration carries the standard notice.
#[test]
fn every_tool_description_carries_the_injection_notice() {
    let dir = manifest_dir().join(HANDLERS_DIR);
    let notice = webfang_mcp::mcp_server::provenance::INJECTION_NOTICE;
    let mut total = 0usize;
    let mut marked = 0usize;
    for entry in std::fs::read_dir(&dir).expect("handlers dir must exist") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let source = read_source(&path);
        for (i, chunk) in tool_chunks(&source).into_iter().enumerate() {
            total += 1;
            if chunk.contains(notice) {
                marked += 1;
            } else {
                panic!(
                    "PROVENANCE GATE: tool registration #{i} in {} lacks the standard \
                     injection notice. Append `INJECTION_NOTICE` wording to its \
                     description (see mcp_server::provenance::INJECTION_NOTICE, \
                     issue #1600).",
                    path.display()
                );
            }
        }
    }
    assert!(
        total >= 36,
        "the 36-tool inventory shrank: found {total} registrations"
    );
    assert_eq!(
        marked, total,
        "every tool registration must carry the notice"
    );
}

/// Gate 3 (wiring sanity): the sanctioned constructors still live in the
/// provenance module — i.e. the gate above is anchored to the right module,
/// and a refactor that moved/deleted it would fail loudly here.
#[test]
fn provenance_module_still_owns_the_sanctioned_constructors() {
    let source = read_source(&manifest_dir().join("src/mcp_server").join("provenance.rs"));
    for required in [
        "pub fn untrusted_text(",
        "pub fn local_text(",
        "pub fn neutralized_error(",
        "CallToolResult::success(vec![Content::text(",
        "pub const INJECTION_NOTICE",
        "pub const MAX_UNTRUSTED_BYTES",
    ] {
        assert!(
            source.contains(required),
            "provenance.rs must still contain {required:?}"
        );
    }
}
