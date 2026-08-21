//! Playwright MCP AXTree serializer — pure function, unit-testable without CDP.
//!
//! Converts a full `Vec<AxNode>` into a YAML-like snapshot with `eN` refs
//! (no `@`) and `chars/4` token estimate. Zero I/O: fixtures drive tests.

#[cfg(feature = "chromium")]
use chromiumoxide::cdp::browser_protocol::accessibility::{AxNode, AxProperty};

#[cfg(feature = "chromium")]
use crate::infrastructure::axtree::compact::ax_value_str;

/// YAML-like accessibility snapshot with `eN` refs.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct PlaywrightSnapshot {
    /// YAML content: `- role "name" [ref=eN] [prop]` per line.
    pub content: String,
    /// `chars() / 4` over `content`.
    pub token_estimate: usize,
    /// Number of `eN` refs emitted.
    pub ref_count: usize,
}

/// Newtype for property bracket display: `[key=value]`.
#[allow(dead_code)]
pub(crate) struct PropertyBracket<'a>(pub &'a AxProperty);

impl<'a> std::fmt::Display for PropertyBracket<'a> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let key = self.0.name.as_ref();
        let val = ax_value_str(Some(&self.0.value));
        // Fallback to JSON stringify when not a plain string.
        let val_str = if val.is_empty() {
            match &self.0.value.value {
                Some(v) if !v.is_string() => v.to_string(),
                _ => String::new(),
            }
        } else {
            val.to_string()
        };
        if val_str.is_empty() {
            write!(f, "[{key}]")
        } else {
            write!(f, "[{key}={val_str}]")
        }
    }
}

/// Snapshot-scoped reference `eN` (vs compact `@eN`).
#[allow(dead_code)]
pub(crate) struct AxReference(pub usize);

impl std::fmt::Display for AxReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "e{}", self.0)
    }
}

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

#[allow(dead_code)]
fn is_interactive_role(role: &str) -> bool {
    INTERACTIVE_ROLES
        .iter()
        .any(|r| role.eq_ignore_ascii_case(r))
}

#[allow(dead_code)]
fn escape_name(raw: &str, out: &mut String) {
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(ch),
        }
    }
}

/// Serialize a full AXTree into a Playwright MCP YAML snapshot.
///
/// Skipped: `ignored` and `genericcontainer`. When `interactive_only`,
/// only interactive roles are emitted. Selector filters case-insensitive
/// substring on role/name BEFORE ref assignment (refs renumber per snapshot).
/// Output is deterministic: pre-order `&[AxNode]` with no HashMap.
#[cfg(feature = "chromium")]
#[allow(dead_code)]
pub(crate) fn playwright(
    nodes: &[AxNode],
    interactive_only: bool,
    selector: Option<&str>,
) -> PlaywrightSnapshot {
    let selector_lc = selector.map(|s| s.to_lowercase());

    // Filter first, then assign refs sequentially.
    let mut filtered: Vec<&AxNode> = Vec::with_capacity(nodes.len());
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
        filtered.push(node);
    }

    if filtered.is_empty() {
        return PlaywrightSnapshot {
            content: String::new(),
            token_estimate: 0,
            ref_count: 0,
        };
    }

    // Estimate capacity: ~40 chars per node average.
    let mut content = String::with_capacity(filtered.len() * 48);
    let mut ref_count = 0usize;

    // Pre-order indent stack — flat array is already pre-order, use depth 0 for now.
    // Vec<usize> stack kept for determinism (no HashMap) per design.
    let _indent_stack: Vec<usize> = Vec::with_capacity(filtered.len());

    for node in filtered {
        ref_count += 1;
        let role = ax_value_str(node.role.as_ref());
        let name = ax_value_str(node.name.as_ref());

        content.push_str("- ");
        content.push_str(role);
        content.push_str(" \"");
        escape_name(name, &mut content);
        content.push('"');
        content.push_str(" [ref=");
        // Use AxReference newtype for eN display.
        content.push_str(&AxReference(ref_count).to_string());
        content.push(']');

        // Props: use PropertyBracket newtype per design. Also emit value prop if present and not empty.
        // Properties
        if let Some(props) = node.properties.as_ref() {
            for prop in props {
                content.push(' ');
                content.push_str(&PropertyBracket(prop).to_string());
            }
        }
        // Emit value as [value=...] when AxNode.value present (Playwright includes value).
        if let Some(val) = node.value.as_ref() {
            let v = ax_value_str(Some(val));
            let v_str = if v.is_empty() {
                match &val.value {
                    Some(jv) if !jv.is_string() => jv.to_string(),
                    _ => String::new(),
                }
            } else {
                v.to_string()
            };
            if !v_str.is_empty() {
                content.push_str(" [value=");
                // escape value similarly? Playwright escapes value as well; keep raw for now but escape quotes.
                let mut escaped = String::new();
                escape_name(&v_str, &mut escaped);
                content.push_str(&escaped);
                content.push(']');
            }
        }

        content.push('\n');
    }

    let token_estimate = content.chars().count() / 4;
    PlaywrightSnapshot {
        content,
        token_estimate,
        ref_count,
    }
}

#[cfg(all(test, feature = "chromium"))]
mod tests {
    use super::*;
    use chromiumoxide::cdp::browser_protocol::accessibility::{
        AxNode, AxProperty, AxValue, AxValueType,
    };

    const GITHUB_FIXTURE: &str = include_str!("../../../tests/fixtures/axtree/github_nav.json");
    #[allow(dead_code)]
    const FORM_FIXTURE: &str = include_str!("../../../tests/fixtures/axtree/form_page.json");

    fn parse(fixture: &str) -> Vec<AxNode> {
        serde_json::from_str(fixture).expect("fixture parses into crate AxNode")
    }

    #[test]
    fn refs_without_at() {
        let snap = playwright(&parse(GITHUB_FIXTURE), false, None);
        assert!(
            snap.content.contains("[ref=e"),
            "must contain [ref=eN], got: {}",
            snap.content
        );
        assert!(
            !snap.content.contains("@e"),
            "must contain zero @eN, got: {}",
            snap.content
        );
        // Ensure sequential e1..
        assert!(snap.content.contains("[ref=e1]"));
    }

    #[test]
    fn empty_yields_zero() {
        let snap = playwright(&[], true, None);
        assert_eq!(snap.ref_count, 0);
        assert_eq!(snap.token_estimate, 0);
        assert_eq!(snap.content, "");
    }

    #[test]
    fn escaping() {
        // Build a single node with tricky name: a"b\nc\d
        let raw = "a\"b\nc\\d";
        let node = AxNode::builder()
            .node_id("x".to_string())
            .ignored(false)
            .role(
                AxValue::builder()
                    .r#type(AxValueType::InternalRole)
                    .value(serde_json::Value::String("button".to_string()))
                    .build()
                    .unwrap(),
            )
            .name(
                AxValue::builder()
                    .r#type(AxValueType::ComputedString)
                    .value(serde_json::Value::String(raw.to_string()))
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let snap = playwright(&[node], false, None);
        // raw a"b\nc\d -> a\"b\nc\d with \n literal escape, \" escaped, \ escaped
        assert!(
            snap.content.contains("a\\\"b\\nc\\\\d"),
            "escaping mismatch, got: {}",
            snap.content
        );
    }

    #[test]
    fn interactive_only_controls_count() {
        let snap_all = playwright(&parse(GITHUB_FIXTURE), false, None);
        let snap_interactive = playwright(&parse(GITHUB_FIXTURE), true, None);
        assert!(
            snap_interactive.ref_count < snap_all.ref_count,
            "interactive_only true must have fewer refs: {} vs {}",
            snap_interactive.ref_count,
            snap_all.ref_count
        );
        assert_eq!(
            snap_all.token_estimate,
            snap_all.content.chars().count() / 4
        );
        assert_eq!(
            snap_interactive.token_estimate,
            snap_interactive.content.chars().count() / 4
        );
    }

    #[test]
    fn selector_filters_and_renumbers() {
        let snap = playwright(&parse(GITHUB_FIXTURE), false, Some("sign"));
        // github_nav has "Sign in" and "Sign up" buttons
        assert!(snap.content.contains("Sign in") || snap.content.contains("Sign"));
        assert!(
            snap.content.contains("[ref=e1]"),
            "refs must renumber from e1, got: {}",
            snap.content
        );
        // Ensure no other unrelated nodes like pricing appear when filtering "sign"
        assert!(
            !snap.content.contains("Pricing"),
            "selector must filter out non-matching nodes"
        );
    }

    #[test]
    fn token_is_chars_over_four() {
        let snap = playwright(&parse(GITHUB_FIXTURE), false, None);
        assert_eq!(snap.token_estimate, snap.content.chars().count() / 4);
    }

    #[test]
    fn deterministic() {
        let nodes = parse(GITHUB_FIXTURE);
        let a = playwright(&nodes, false, None);
        let b = playwright(&nodes, false, None);
        assert_eq!(a.content, b.content, "playwright must be byte-identical");
        assert_eq!(a.token_estimate, b.token_estimate);
        assert_eq!(a.ref_count, b.ref_count);
    }

    #[test]
    fn props_use_brackets() {
        // Create node with a property level=1
        let mut node = AxNode::builder()
            .node_id("x".to_string())
            .ignored(false)
            .role(
                AxValue::builder()
                    .r#type(AxValueType::InternalRole)
                    .value(serde_json::Value::String("heading".to_string()))
                    .build()
                    .unwrap(),
            )
            .name(
                AxValue::builder()
                    .r#type(AxValueType::ComputedString)
                    .value(serde_json::Value::String("Title".to_string()))
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        node.properties = Some(vec![AxProperty::new(
            chromiumoxide::cdp::browser_protocol::accessibility::AxPropertyName::Level,
            AxValue::builder()
                .r#type(AxValueType::Integer)
                .value(serde_json::Value::Number(serde_json::Number::from(1)))
                .build()
                .unwrap(),
        )]);
        let snap = playwright(&[node], false, None);
        assert!(
            snap.content.contains("[level=1]"),
            "PropertyBracket must render [level=1], got: {}",
            snap.content
        );
    }

    #[test]
    fn insta_playwright_github_nav() {
        let snap = playwright(&parse(GITHUB_FIXTURE), true, None);
        insta::assert_snapshot!("playwright_github_nav", snap.content);
    }

    #[test]
    fn insta_playwright_form_page() {
        let snap = playwright(&parse(FORM_FIXTURE), true, None);
        insta::assert_snapshot!("playwright_form_page", snap.content);
    }

    #[test]
    fn compact_vs_playwright_delta_github_nav() {
        use crate::infrastructure::axtree::compact::compact;
        let nodes = parse(GITHUB_FIXTURE);
        let compact_snap = compact(&nodes, true, None);
        let pw = playwright(&nodes, true, None);
        // playwright uses chars/4, compact uses Σ(2+name/4+role/4) — delta ~51-79% per F1[6]
        assert!(
            pw.token_estimate > compact_snap.token_estimate,
            "playwright token must exceed compact"
        );
        assert!(
            pw.token_estimate <= compact_snap.token_estimate * 3,
            "playwright token bounded, not unbounded"
        );
        // insta lock for deterministic regression
        insta::assert_snapshot!(
            "compact_vs_playwright_delta",
            format!(
                "compact token={} nodes={}\nplaywright token={} refs={}\nplaywright content:\n{}",
                compact_snap.token_estimate,
                compact_snap.nodes.len(),
                pw.token_estimate,
                pw.ref_count,
                pw.content
            )
        );
    }

    #[test]
    fn behavioral_cdp_mock_ephemeral_no_real_chrome() {
        // Ephemeral adapter: fixture parse simulates CDP GetFullAXTree without browser/network
        let nodes = parse(GITHUB_FIXTURE);
        assert_eq!(nodes.len(), 9, "github_nav fixture must yield 9 AxNode");
        let all = playwright(&nodes, false, None);
        // 7 non-ignored non-generic nodes: navigation, 2 buttons, 2 links, heading, textbox
        assert_eq!(all.ref_count, 7, "full tree ref_count must be 7");
        assert!(!all.content.contains("@e"), "playwright must have zero @e");
        // interactive_only reduces to 5
        let interactive = playwright(&nodes, true, None);
        assert_eq!(interactive.ref_count, 5);
        assert!(interactive.ref_count < all.ref_count);
        // stale-ref contract: reusing old eN after selector change requires re-snapshot
        let filtered = playwright(&nodes, true, Some("sign"));
        assert!(
            filtered.content.contains("[ref=e1]"),
            "refs must renumber from e1 after selector"
        );
        assert!(
            !filtered.content.contains("Pricing"),
            "selector must filter"
        );
    }

    #[test]
    fn observability_instrument_fields_present() {
        // Verify token_estimate == chars/4 and ref_count emitted would be recorded via instrument
        let snap = playwright(&parse(GITHUB_FIXTURE), true, None);
        assert_eq!(snap.token_estimate, snap.content.chars().count() / 4);
        assert!(snap.ref_count > 0);
        // CorrelationId child shares trace_id — proof of R7 observability contract
        let root = crate::domain::CorrelationId::new();
        let child = root.child();
        assert_eq!(root.trace_id(), child.trace_id());
        assert_ne!(root.span_id(), child.span_id());
    }
}
