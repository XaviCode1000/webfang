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

// ========================================================================
// Gate 4 — the POLICY FILE is reachable from every surface that has to
// obey it (PI-2, issue #1615).
// ========================================================================

/// Repository root, two levels up from `crates/webfang_mcp`.
fn repo_root() -> PathBuf {
    manifest_dir()
        .parent()
        .and_then(Path::parent)
        .expect("crate lives at <repo>/crates/webfang_mcp")
        .to_path_buf()
}

/// The agent-facing surface is `AGENTS.md`, not the tool description.
///
/// PI-2 recorded that `docs/security/prompt-injection-policy.md` had no
/// references from code, tests, CI or agent surfaces, and that the auditor
/// "could not confirm an `AGENTS.md` Layer-2 reference" even though Layer 2
/// was documented there in prose. Prose that no check reads is what a
/// refactor deletes, and the next auditor reads the absence as a finding again.
///
/// Gates 1–3 above cover the CODE surface. This covers the two that are prose
/// by nature: the agent instructions, and the policy's own reachability. Both
/// are one-line assertions, and the failure message names the file to edit —
/// which is the whole value, since a gate nobody knows about is a gate nobody
/// fixes.
#[test]
fn the_policy_is_reachable_from_the_agent_surface_and_from_agents_md() {
    let policy_rel = "docs/security/prompt-injection-policy.md";
    let policy = repo_root().join(policy_rel);
    assert!(
        policy.exists(),
        "the policy this gate enforces must exist at {policy_rel}"
    );

    let agents =
        std::fs::read_to_string(repo_root().join("AGENTS.md")).expect("gate must read AGENTS.md");
    assert!(
        agents.contains(policy_rel),
        "AGENTS.md must name {policy_rel}: the agent-facing Layer-2 rules point at \
         it, and without that reference the policy is only reachable by someone \
         who already knows it exists"
    );
}

/// The policy must state the rules it exists to enforce, not merely exist.
///
/// PI-2's substance is reachability PLUS content: a policy that is linked from
/// `AGENTS.md` but says nothing is still unenforced. `Regla 0` is the one every
/// tool description and the export sidecar (#1615 PI-9) cite, so it is the one
/// whose disappearance would silently strip the meaning out of all of them.
///
/// NOTE: the audit's `M5` label names a section of the AUDIT REPORT's mitigation
/// plan, not of this policy file — the policy numbers its rules `Regla N`. An
/// earlier version of this gate asserted on the literal `M5` and failed, which
/// is why the assertion names the real headings.
#[test]
fn the_policy_states_the_rules_its_referrers_cite() {
    let policy =
        std::fs::read_to_string(repo_root().join("docs/security/prompt-injection-policy.md"))
            .expect("gate must read the policy");
    assert!(
        policy.len() > 500,
        "the policy must have content; a stub cannot carry a rule"
    );
    for rule in ["Regla 0", "Regla 1", "Regla 2", "Regla 3"] {
        assert!(
            policy.contains(rule),
            "the policy must keep {rule:?}: every tool description and the export \
             provenance sidecar cite this policy, so a rule that disappears leaves \
             all of them citing nothing"
        );
    }
    // The English sentence the code side carries verbatim, so the policy and
    // `provenance::INJECTION_NOTICE` cannot drift into saying different things.
    assert!(
        policy.contains("data, no instrucciones"),
        "the policy must state Regla 0 in the form the code cites: tool output is \
         data, not instructions"
    );
}
