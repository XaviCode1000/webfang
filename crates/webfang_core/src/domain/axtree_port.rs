//! AXTree port — domain trait for accessibility-tree snapshots.
//!
//! Owns the pure DTOs (`SnapshotFormat`, `CompactNode`, `CompactSnapshot`),
//! the `RawAxNodeView` trait abstraction, the `compact` serializer, and
//! the `AxTreePort` trait. The I/O implementations (chromiumoxide CDP
//! fetcher) live in `infrastructure::axtree` and impl this trait; today the
//! only consumers are the `infrastructure::axtree` free functions
//! (`fetch_axtree_snapshot`, `fetch_playwright_snapshot`) called by the MCP
//! axtree handler. The trait remains the DI seam for future application use.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::error::ScraperError;

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Domain port for fetching raw accessibility trees.
///
/// The I/O implementations live in `infrastructure::axtree`
/// (`ChromiumoxideAxTreeAdapter`). The trait is the DI seam for application
/// code that needs raw AX nodes; current snapshot consumers go through the
/// infrastructure free functions instead.
///
/// `#[cfg(feature = "chromium")]` impl spawns headless Chromium via CDP;
/// non-chromium stub returns an error.
pub trait AxTreePort: Send + Sync {
    /// Fetch the raw AXTree nodes for a URL.
    ///
    /// Returns the nodes as trait objects (`Box<dyn RawAxNodeView>`) so the
    /// domain `compact` function can process them without depending on
    /// browser-specific types.
    fn fetch_raw_axtree<'a>(
        &'a self,
        url: &'a Url,
    ) -> BoxFuture<'a, Result<Vec<Box<dyn RawAxNodeView>>, ScraperError>>;
}

/// Snapshot serialization formats (spec R3).
///
/// Only [`Compact`](Self::Compact) is implemented in this slice; `playwright-mcp`
/// returns an honest unsupported error (ai.rs precedent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SnapshotFormat {
    /// Interactive-only `@eN`-referenced node list with a `token_estimate`.
    #[default]
    Compact,
    /// Playwright MCP AXSnapshot format — deferred to a follow-up change.
    PlaywrightMcp,
}

/// A single compact node: `@eN` ref, accessible name, and role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactNode {
    /// Snapshot-scoped reference (`@e1`, `@e2`, …) — valid ONLY within the
    /// snapshot that created it (RDD causal invariant).
    #[serde(rename = "ref")]
    pub r#ref: String,
    /// Accessible name.
    pub name: String,
    /// Accessible role.
    pub role: String,
}

/// Compact accessibility snapshot — interactive nodes plus a token estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactSnapshot {
    /// Emitted nodes, each with a snapshot-scoped `@eN` ref.
    pub nodes: Vec<CompactNode>,
    /// `Σ(2 + name_chars/4 + role_chars/4)` over the emitted nodes.
    pub token_estimate: usize,
}

/// Domain abstraction over a raw AXTree node.
///
/// The concrete AXTree node type comes from chromiumoxide's CDP types in the
/// chromium build; this trait lets `compact` (and any future domain logic)
/// operate on a stable interface without depending on browser-specific types.
pub trait RawAxNodeView {
    /// Whether the node is marked ignored by the accessibility tree.
    fn is_ignored(&self) -> bool;
    /// The node's role as a string, or `None` when absent.
    fn role_str(&self) -> Option<&str>;
    /// The node's accessible name as a string, or `None` when absent.
    fn name_str(&self) -> Option<&str>;
}

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
/// When `interactive_only`, only nodes whose role is in the interactive
/// roles set (button, link, textbox, etc.) are emitted. When `selector` is
/// present, only nodes whose name or role contains it (case-insensitive
/// substring) are kept.
///
/// `@eN` refs are assigned in emission order and are valid ONLY within the
/// returned snapshot (RDD causal invariant).
pub fn compact(
    nodes: &[Box<dyn RawAxNodeView>],
    interactive_only: bool,
    selector: Option<&str>,
) -> CompactSnapshot {
    let selector_lc = selector.map(str::to_lowercase);
    let mut compact_nodes: Vec<CompactNode> = Vec::with_capacity(nodes.len());

    for node in nodes.iter() {
        if node.is_ignored() {
            continue;
        }
        let role = node.role_str().unwrap_or("");
        if role.eq_ignore_ascii_case("genericcontainer") {
            continue;
        }
        let name = node.name_str().unwrap_or("");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ScraperError;
    use std::sync::Arc;
    use url::Url;

    // ========================================================================
    // FakeNode — local RawAxNodeView impl for tests
    // ========================================================================

    struct FakeNode {
        ignored: bool,
        role: Option<String>,
        name: Option<String>,
    }

    impl RawAxNodeView for FakeNode {
        fn is_ignored(&self) -> bool {
            self.ignored
        }
        fn role_str(&self) -> Option<&str> {
            self.role.as_deref()
        }
        fn name_str(&self) -> Option<&str> {
            self.name.as_deref()
        }
    }

    fn n(ignored: bool, role: &str, name: &str) -> Box<dyn RawAxNodeView> {
        Box::new(FakeNode {
            ignored,
            role: if role.is_empty() {
                None
            } else {
                Some(role.to_string())
            },
            name: if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            },
        })
    }

    struct FakeAx;

    impl AxTreePort for FakeAx {
        fn fetch_raw_axtree<'a>(
            &'a self,
            url: &'a Url,
        ) -> BoxFuture<'a, Result<Vec<Box<dyn RawAxNodeView>>, ScraperError>> {
            let u = url.clone();
            Box::pin(async move {
                let node: Box<dyn RawAxNodeView> = Box::new(FakeNode {
                    ignored: false,
                    role: Some("button".to_string()),
                    name: Some(format!("fake:{u}")),
                });
                Ok(vec![node])
            })
        }
    }

    #[tokio::test]
    async fn fetch_raw_axtree_returns_nodes() {
        let port = FakeAx;
        let url = Url::parse("https://example.com").unwrap();
        let nodes = port.fetch_raw_axtree(&url).await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].role_str(), Some("button"));
        assert!(nodes[0].name_str().unwrap().contains("example.com"));

        let url2 = Url::parse("https://rust-lang.org").unwrap();
        let nodes2 = port.fetch_raw_axtree(&url2).await.unwrap();
        assert!(nodes2[0].name_str().unwrap().contains("rust-lang.org"));
    }

    #[test]
    fn axtree_port_is_object_safe() {
        fn assert_dyn(_: &dyn AxTreePort) {}
        let p = FakeAx;
        assert_dyn(&p);
        let _: Arc<dyn AxTreePort> = Arc::new(FakeAx);
    }

    // ========================================================================
    // compact() tests — moved from infrastructure::axtree::compact in
    // sub-slice 3.A.2-followup.A.
    // ========================================================================

    #[test]
    fn interactive_only_emits_interactive_nodes_with_sequential_refs() {
        let nodes: Vec<Box<dyn RawAxNodeView>> = vec![
            n(false, "button", "Sign in"),
            n(false, "link", "Home"),
            n(false, "textbox", "Username"),
            n(false, "heading", "Welcome"),
        ];
        let snapshot = compact(&nodes, true, None);
        assert_eq!(snapshot.nodes.len(), 3);
        for (i, c) in snapshot.nodes.iter().enumerate() {
            assert_eq!(c.r#ref, format!("@e{}", i + 1), "refs must be sequential");
        }
        let roles: Vec<_> = snapshot.nodes.iter().map(|c| c.role.as_str()).collect();
        assert_eq!(roles, ["button", "link", "textbox"]);
    }

    #[test]
    fn token_estimate_matches_formula() {
        let nodes: Vec<Box<dyn RawAxNodeView>> = vec![
            n(false, "button", "Sign in"),   // 2 + 7/4 + 6/4 = 2 + 1 + 1 = 4
            n(false, "link", "Home"),        // 2 + 4/4 + 4/4 = 2 + 1 + 1 = 4
            n(false, "textbox", "Username"), // 2 + 8/4 + 7/4 = 2 + 2 + 1 = 5
        ];
        let snapshot = compact(&nodes, true, None);
        assert_eq!(snapshot.token_estimate, 4 + 4 + 5);
    }

    #[test]
    fn empty_tree_yields_empty_snapshot_with_zero_estimate() {
        let snapshot = compact(&[], true, None);
        assert!(snapshot.nodes.is_empty());
        assert_eq!(snapshot.token_estimate, 0);
    }

    #[test]
    fn full_tree_includes_non_interactive_roles_when_requested() {
        let nodes: Vec<Box<dyn RawAxNodeView>> = vec![
            n(false, "heading", "Welcome"),
            n(false, "navigation", "Main"),
        ];
        let snapshot = compact(&nodes, false, None);
        let roles: Vec<_> = snapshot.nodes.iter().map(|c| c.role.as_str()).collect();
        assert!(roles.contains(&"heading"));
        assert!(roles.contains(&"navigation"));
    }

    #[test]
    fn selector_filters_by_name_or_role_substring() {
        let nodes: Vec<Box<dyn RawAxNodeView>> = vec![
            n(false, "button", "Submit"),
            n(false, "textbox", "Username"),
            n(false, "button", "Cancel"),
        ];
        let snap_user = compact(&nodes, true, Some("user"));
        assert_eq!(snap_user.nodes.len(), 1);
        assert_eq!(snap_user.nodes[0].name, "Username");

        let snap_butt = compact(&nodes, true, Some("butt"));
        assert_eq!(snap_butt.nodes.len(), 2);
        assert!(snap_butt.nodes.iter().all(|c| c.role == "button"));
    }

    #[test]
    fn ignored_and_generic_container_nodes_are_skipped() {
        let nodes: Vec<Box<dyn RawAxNodeView>> = vec![
            n(true, "generic", "ignored"),
            n(false, "genericcontainer", "wrapper"),
            n(false, "button", "OK"),
        ];
        let snapshot = compact(&nodes, false, None);
        assert_eq!(snapshot.nodes.len(), 1);
        assert_eq!(snapshot.nodes[0].role, "button");
    }
}
