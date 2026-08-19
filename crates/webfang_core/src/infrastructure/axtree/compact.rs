//! Compact AXTree serializer — pure function, unit-testable without CDP (#788).
//!
//! Converts a full `Vec<AxNode>` (from CDP `Accessibility.getFullAXTree`) into
//! a compact snapshot of interactive nodes with `@eN` refs and a
//! `token_estimate`. Zero I/O: fixtures drive the tests via `include_str!`.

use chromiumoxide::cdp::browser_protocol::accessibility::{AxNode, AxValue};

use crate::infrastructure::axtree::{CompactNode, CompactSnapshot};

/// Interactive roles kept when `interactive_only` is true (spec R2).
const INTERACTIVE_ROLES: &[&str] = &[
    "button",
    "link",
    "textbox",
    "checkbox",
    "radio",
    "combobox",
    "listbox",
    "menuitem",
    "tab",
    "switch",
    "slider",
    "spinbutton",
    "searchbox",
    "treeitem",
    "option",
];

/// Serialize a full AXTree into a compact snapshot.
///
/// Skipped unconditionally: `ignored` nodes and `genericcontainer` wrappers.
/// When `interactive_only`, only nodes whose role is in [`INTERACTIVE_ROLES`]
/// are emitted. When `selector` is present, only nodes whose name or role
/// contains it (case-insensitive substring) are kept.
///
/// `@eN` refs are assigned in emission order and are valid ONLY within the
/// returned snapshot (RDD causal invariant).
pub(crate) fn compact(
    nodes: &[AxNode],
    interactive_only: bool,
    selector: Option<&str>,
) -> CompactSnapshot {
    let selector_lc = selector.map(str::to_lowercase);
    let mut compact_nodes: Vec<CompactNode> = Vec::with_capacity(nodes.len());

    for node in nodes {
        if node.ignored {
            continue;
        }
        let role = ax_value_str(node.role.as_ref());
        if role.eq_ignore_ascii_case("genericcontainer") {
            continue;
        }
        let name = ax_value_str(node.name.as_ref());
        if interactive_only && !is_interactive_role(role) {
            continue;
        }
        if let Some(sel) = &selector_lc {
            if !name.to_lowercase().contains(sel.as_str())
                && !role.to_lowercase().contains(sel.as_str())
            {
                continue;
            }
        }
        compact_nodes.push(CompactNode {
            r#ref: format!("@e{}", compact_nodes.len() + 1),
            name: name.to_string(),
            role: role.to_string(),
        });
    }

    let token_estimate = compact_nodes
        .iter()
        .map(|n| 2 + n.name.chars().count() / 4 + n.role.chars().count() / 4)
        .sum();

    CompactSnapshot {
        nodes: compact_nodes,
        token_estimate,
    }
}

/// Whether `role` is in the interactive set (case-insensitive).
fn is_interactive_role(role: &str) -> bool {
    INTERACTIVE_ROLES
        .iter()
        .any(|r| role.eq_ignore_ascii_case(r))
}

/// Extract the string payload of an `AxValue` (empty when absent).
fn ax_value_str(value: Option<&AxValue>) -> &str {
    value
        .and_then(|v| v.value.as_ref())
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    const GITHUB_FIXTURE: &str = include_str!("../../../tests/fixtures/axtree/github_nav.json");
    const FORM_FIXTURE: &str = include_str!("../../../tests/fixtures/axtree/form_page.json");

    fn parse(fixture: &str) -> Vec<AxNode> {
        serde_json::from_str(fixture).expect("fixture parses into crate AxNode")
    }

    #[test]
    fn interactive_only_emits_interactive_nodes_with_sequential_refs() {
        let snapshot = compact(&parse(GITHUB_FIXTURE), true, None);
        assert!(!snapshot.nodes.is_empty());
        for (i, n) in snapshot.nodes.iter().enumerate() {
            assert_eq!(n.r#ref, format!("@e{}", i + 1), "refs must be sequential");
            assert!(
                is_interactive_role(&n.role),
                "only interactive nodes may carry refs, got role {}",
                n.role
            );
        }
        let roles: Vec<_> = snapshot.nodes.iter().map(|n| n.role.as_str()).collect();
        assert_eq!(roles, ["button", "button", "link", "link", "textbox"]);
    }

    #[test]
    fn token_estimate_matches_formula_and_stays_below_bound() {
        let snapshot = compact(&parse(GITHUB_FIXTURE), true, None);
        // Formula: Σ(2 + name_chars/4 + role_chars/4); github fixture sums to 23.
        assert_eq!(snapshot.token_estimate, 23);
        // Compact cost stays in tens of tokens — far below full-HTML/screenshot
        // (thousands) — per the RDD causal invariant.
        assert!(
            snapshot.token_estimate <= 48,
            "token_estimate must respect the compact bound"
        );
    }

    #[test]
    fn empty_tree_yields_empty_snapshot_with_zero_estimate() {
        let snapshot = compact(&[], true, None);
        assert!(snapshot.nodes.is_empty());
        assert_eq!(snapshot.token_estimate, 0);
    }

    #[test]
    fn full_tree_includes_non_interactive_roles_when_requested() {
        let snapshot = compact(&parse(GITHUB_FIXTURE), false, None);
        let roles: Vec<_> = snapshot.nodes.iter().map(|n| n.role.as_str()).collect();
        assert!(
            roles.contains(&"heading"),
            "heading must appear when interactive_only=false"
        );
        assert!(roles.contains(&"navigation"));
    }

    #[test]
    fn selector_filters_by_name_or_role_substring() {
        let snapshot = compact(&parse(FORM_FIXTURE), true, Some("user"));
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].name, "Username");

        let snapshot = compact(&parse(FORM_FIXTURE), true, Some("butt"));
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(
            snapshot.nodes[0].role, "button",
            "role substring must match"
        );
    }

    #[test]
    fn ignored_and_generic_container_nodes_are_skipped() {
        // form_page has an ignored decorative node; github has a genericcontainer root.
        let snapshot = compact(&parse(FORM_FIXTURE), false, None);
        assert!(snapshot.nodes.iter().all(|n| n.role != "generic"));
        let snapshot = compact(&parse(GITHUB_FIXTURE), false, None);
        assert!(snapshot.nodes.iter().all(|n| n.role != "genericcontainer"));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // exercises the real chromiumoxide CDP deserialization stack (#775 precedent)
    fn fixture_deserializes_through_crate_axnode() {
        // Proves the crate `AxNode` shape is reused as-is (spec R1 edge): the
        // fixture is a real CDP `Accessibility.getFullAXTree` payload.
        let nodes = parse(GITHUB_FIXTURE);
        assert_eq!(nodes.len(), 9);
        assert!(nodes.iter().any(|n| n.role.as_ref().is_some()));
    }
}
