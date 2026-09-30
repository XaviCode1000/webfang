//! The MCP public-surface policy, made checkable (issue #1614).
//!
//! `docs/src/mcp-public-surface-policy.md` is the decision; this file is the
//! enforcement of the part of it that can be enforced without a human in the
//! loop, and `scripts/check_mcp_public_surface.sh` is the enforcement of the
//! rest (the version marker).
//!
//! **What this pins.** For every tool the router registers, the compatibility
//! -bearing dimensions of the advertised `inputSchema`:
//!
//! | dimension | why it is compatibility-bearing |
//! |---|---|
//! | the tool inventory | a removed tool is a dead client call (S2) |
//! | `additionalProperties` | relaxing it stops `deny_unknown_fields` (S2) |
//! | a property's presence | added-required is S2, added-optional is S1 |
//! | `required` membership | an existing request stops being valid (S2) |
//! | `type` (incl. the `null` union) | nullability tightening is S2 |
//! | `default` | a move here without a matching runtime move is drift |
//! | `minimum` / `maximum` | tightening is S2 |
//! | the constant set of a nested enum `$defs` entry | a variant reaching the wire |
//!
//! **What this deliberately does NOT pin**, so the coverage claim stays honest:
//!
//! - **Descriptions.** Prose, and S0 by the policy. Pinning them would make
//!   every wording tweak a red test, which is how a gate gets ignored. The
//!   seven bridged schemas ARE pinned byte-for-byte by
//!   `mcp_advertised_schema_snapshot_test` (#1612), descriptions included.
//! - **Response payloads.** BC-08, explicitly deferred by the policy (§3.2).
//! - **Whether `contract_version` moved.** That needs the previous committed
//!   state, which a test cannot see; the shell guard does it.
//!
//! Run: `cargo nextest run -p webfang_mcp --test public_surface_policy_test`

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde_json::Value;
use webfang_mcp::mcp_server::handlers::build_tool_router;

/// Wire name of the env-gated panic probe (`mcp_server::test_probe`).
///
/// Duplicated here because `test_probe` is `pub(crate)`, so an integration
/// test cannot import `PANIC_PROBE_TOOL_NAME`. If the probe is ever renamed,
/// this exclusion misses the rename and `tool_inventory_is_pinned` fails — a
/// loud, one-line fix, which is the right failure for a value that guards an
/// exclusion.
const PANIC_PROBE_TOOL_NAME: &str = "test_panic_probe";

/// The generated surface fixture. `CARGO_MANIFEST_DIR`-anchored so the path
/// does not depend on the test runner's working directory.
const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/mcp_public_surface.tsv"
);

/// The version marker, deliberately a SEPARATE file from the fixture.
///
/// If `contract_version` lived inside the fixture, re-recording the fixture
/// would carry the version with it and the "did the marker move?" question
/// would answer itself. Splitting them means a re-record can never bump the
/// version by accident: the only way past the guard is to edit the version file
/// on purpose, which is the decision point the policy wants a human at.
const VERSION_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/mcp_public_surface.version"
);

/// Set to `1` to rewrite [`FIXTURE`] from the live surface.
///
/// Record mode exists so the surface can be re-derived mechanically instead of
/// hand-edited. It NEVER touches [`VERSION_FILE`] — that is the whole point of
/// the split above, and it is why re-recording a breaking change leaves this
/// suite green while `check_mcp_public_surface.sh` stays red until someone
/// classifies the change and moves the marker.
///
/// CI must never set this. The sanctioned entry point is
/// `scripts/check_mcp_public_surface.sh --record`.
const RECORD_ENV: &str = "WEBFANG_MCP_SURFACE_RECORD";

// ---------------------------------------------------------------------------
// Fixture format
// ---------------------------------------------------------------------------

/// A parsed `[properties]` row: the compatibility dimensions of one property.
///
/// Field order matches the fixture columns, and the class labels in
/// [`diff_rows`] are the policy's §3.1 table.
#[derive(Debug, Default, PartialEq, Eq)]
struct PropertyRow {
    /// The property name on the wire.
    name: String,
    /// Whether the tool advertises it in `required`.
    required: bool,
    /// Normalized JSON type, with the `null` union rendered as `inner|null`.
    ty: String,
    /// Compact JSON of `default`, or `-` when the property advertises none.
    default: String,
    /// Inclusive lower bound, or `-`.
    minimum: String,
    /// Inclusive upper bound, or `-`.
    maximum: String,
}

/// The whole fixture, parsed into its three sections.
#[derive(Debug, Default)]
struct Surface {
    /// `[tools]`: `<tool>\t<additionalProperties>`, sorted by tool.
    tools: Vec<(String, String)>,
    /// `[properties]`: keyed by `(tool, property)`, so a diff is per-row.
    properties: BTreeMap<(String, String), PropertyRow>,
    /// `[defs]`: `<tool>\t<def name>\t<const,comma,list>`, sorted.
    defs: Vec<(String, String, String)>,
}

impl Surface {
    /// Parse the tab-separated fixture. Blank lines, `#` comments and unknown
    /// sections are skipped so the file can carry an explanatory header.
    fn parse(text: &str) -> Self {
        let mut surface = Self::default();
        let mut section = String::new();
        for line in text.lines() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_owned();
                continue;
            }
            let mut fields = line.split('\t');
            let first = fields.next().unwrap_or_default().to_owned();
            match section.as_str() {
                "tools" => {
                    let additional = fields.next().unwrap_or("-").to_owned();
                    surface.tools.push((first, additional));
                },
                "properties" => {
                    // Columns: <tool> <property> <optional|required> <type>
                    //           <default> <min> <max>.
                    let property = fields.next().unwrap_or_default().to_owned();
                    let row = PropertyRow {
                        name: property.clone(),
                        required: fields.next().unwrap_or_default() == "required",
                        ty: fields.next().unwrap_or("-").to_owned(),
                        default: fields.next().unwrap_or("-").to_owned(),
                        minimum: fields.next().unwrap_or("-").to_owned(),
                        maximum: fields.next().unwrap_or("-").to_owned(),
                    };
                    surface.properties.insert((first, property), row);
                },
                "defs" => {
                    let name = fields.next().unwrap_or_default().to_owned();
                    let consts = fields.next().unwrap_or("-").to_owned();
                    surface.defs.push((first, name, consts));
                },
                _ => {},
            }
        }
        surface
    }
}

/// Read the checked-in fixture, panicking with its path when it is missing —
/// a missing manifest is a failed gate, never a skipped one.
fn read_fixture() -> String {
    std::fs::read_to_string(FIXTURE_PATH).unwrap_or_else(|e| {
        panic!(
            "public-surface fixture unreadable at {FIXTURE_PATH}: {e}\n\
             regenerate with: scripts/check_mcp_public_surface.sh --record"
        )
    })
}

// ---------------------------------------------------------------------------
// Deriving the live surface
// ---------------------------------------------------------------------------

/// Normalize a property's advertised `type` into one column.
///
/// A nullable property arrives as the array `["<inner>", "null"]`; rendering
/// that union as `<inner>|null` makes a nullability change a one-column diff
/// instead of a reordering.
fn render_type(schema: &Value) -> String {
    match schema.get("type") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            let mut names: Vec<String> = items
                .iter()
                .map(|item| match item {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect();
            names.sort();
            names.dedup();
            names.join("|")
        },
        _ => "-".to_owned(),
    }
}

/// Compact JSON for a scalar-or-default value, or `-` when absent.
fn render_scalar(value: Option<&Value>) -> String {
    match value {
        None => "-".to_owned(),
        Some(Value::String(s)) => format!("\"{s}\""),
        Some(other) => other.to_string(),
    }
}

/// Every string constant reachable inside a nested `$defs` entry, sorted.
///
/// schemars renders a unit-variant enum as `oneOf: [{const: …}, …]` rather
/// than `enum: […]`, so both spellings are walked. A def with no constants
/// (a nested object, e.g. `McpUrl`) renders as `-`: its SHAPE is already pinned
/// byte-for-byte by the #1612 snapshot for the bridged tools, and duplicating
/// object bodies here would only add churn that says nothing new.
fn const_set(def: &Value) -> String {
    let mut found: Vec<String> = Vec::new();
    fn walk(value: &Value, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    if (key == "const" || key == "enum") && child.is_string() {
                        out.push(child.as_str().unwrap_or_default().to_owned());
                    } else if key == "enum" {
                        if let Value::Array(items) = child {
                            out.extend(items.iter().filter_map(Value::as_str).map(str::to_owned));
                        }
                    }
                    walk(child, out);
                }
            },
            Value::Array(items) => items.iter().for_each(|item| walk(item, out)),
            _ => {},
        }
    }
    walk(def, &mut found);
    found.sort();
    found.dedup();
    if found.is_empty() {
        "-".to_owned()
    } else {
        found.join(",")
    }
}

/// Derive the whole live surface from the production assembly path.
///
/// The input is [`build_tool_router`] — the same one the server registers, with
/// the same `apply_overrides` bridge applied — so this reads what a client is
/// actually served. A surface derived from the params structs would pin this
/// test's own idea of the schema instead.
fn live_surface() -> Surface {
    let mut surface = Surface::default();
    let router = build_tool_router();
    for (name, route) in &router.map {
        // The env-gated panic probe (#1626 PC-3) is not a product tool and is
        // registered only when an operator opts in; it is excluded by the
        // constant rather than by a magic string, so a rename moves the
        // exclusion with it.
        if name == PANIC_PROBE_TOOL_NAME {
            continue;
        }
        let schema = Value::Object(route.attr.input_schema.as_ref().clone());
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();

        surface.tools.push((
            name.to_string(),
            schema
                .get("additionalProperties")
                .map_or_else(|| "-".to_owned(), ToString::to_string),
        ));

        if let Some(Value::Object(defs)) = schema.get("$defs") {
            for (def_name, def) in defs {
                surface
                    .defs
                    .push((name.to_string(), def_name.to_string(), const_set(def)));
            }
        }

        let Some(Value::Object(properties)) = schema.get("properties") else {
            continue;
        };
        for (property, rendered) in properties {
            let property = property.as_str();
            let row = PropertyRow {
                name: property.to_owned(),
                required: required.contains(&property),
                ty: render_type(rendered),
                default: render_scalar(rendered.get("default")),
                minimum: render_scalar(rendered.get("minimum")),
                maximum: render_scalar(rendered.get("maximum")),
            };
            surface
                .properties
                .insert((name.to_string(), row.name.to_string()), row);
        }
    }
    surface.tools.sort();
    surface.defs.sort();
    surface
}

/// Render the live surface into the fixture's exact on-disk form.
///
/// Deterministic by construction: every section is built from a sorted
/// container, so a re-record that changes nothing is a byte-identical file and
/// `git diff` stays empty instead of reshuffling.
fn render(surface: &Surface) -> String {
    let mut out = String::new();
    out.push_str("# MCP public-surface fixture — generated, do NOT hand-edit.\n");
    out.push_str("# Policy: docs/src/mcp-public-surface-policy.md (§3.1 classification table).\n");
    out.push_str("# Regenerate: scripts/check_mcp_public_surface.sh --record\n");
    out.push_str("# A changed row is a compatibility change; the version marker is the\n");
    out.push_str("# separate file mcp_public_surface.version, and the shell guard fails\n");
    out.push_str("# until it moves. Descriptions are intentionally NOT here (S0 prose).\n");
    let _ = writeln!(
        out,
        "# tools: {}  properties: {}  defs: {}",
        surface.tools.len(),
        surface.properties.len(),
        surface.defs.len()
    );

    out.push_str("\n[tools]\n");
    out.push_str("# <tool>\t<additionalProperties>\n");
    for (tool, additional) in &surface.tools {
        let _ = writeln!(out, "{tool}\t{additional}");
    }

    out.push_str("\n[properties]\n");
    out.push_str("# <tool>\t<property>\t<optional|required>\t<type>\t<default>\t<min>\t<max>\n");
    for ((tool, property), row) in &surface.properties {
        let _ = writeln!(
            out,
            "{tool}\t{}\t{}\t{}\t{}\t{}\t{}",
            property,
            if row.required { "required" } else { "optional" },
            row.ty,
            row.default,
            row.minimum,
            row.maximum
        );
    }

    out.push_str("\n[defs]\n");
    out.push_str("# <tool>\t<$defs entry>\t<const,comma,list>\n");
    for (tool, def, consts) in &surface.defs {
        let _ = writeln!(out, "{tool}\t{def}\t{consts}");
    }
    out
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

/// In record mode, rewrite the fixture and stop. The version marker is not
/// touched, by construction — see [`VERSION_FILE`].
fn record_if_requested() -> bool {
    if std::env::var(RECORD_ENV).as_deref() != Ok("1") {
        return false;
    }
    std::fs::write(FIXTURE_PATH, render(&live_surface()))
        .unwrap_or_else(|e| panic!("cannot write public-surface fixture at {FIXTURE_PATH}: {e}"));
    println!("recorded public-surface fixture -> {FIXTURE_PATH}");
    println!("contract_version NOT touched: if a row moved, classify it per policy §3.1 and edit");
    println!("{VERSION_PATH} deliberately.");
    true
}

/// Describe every difference between two row maps, with the policy class of
/// each dimension that moved.
///
/// The class labels are the point: a red test that only says "values differ"
/// gets re-recorded, while one that says "narrowed (S2/breaking)" gets a
/// decision.
fn diff_rows(
    live: &BTreeMap<(String, String), PropertyRow>,
    pinned: &BTreeMap<(String, String), PropertyRow>,
) -> String {
    let mut report = String::new();
    for (key, row) in live {
        match pinned.get(key) {
            None => {
                let _ = writeln!(
                    report,
                    "  + {}/{}: ADDED PROPERTY — {} (policy §3.1: added-and-optional is S1/minor, added-and-required is S2/breaking)",
                    key.0, key.1, row.required
                );
            },
            Some(old) if old != row => {
                let (tool, property) = key;
                let _ = writeln!(report, "  ~ {tool}/{property} changed:");
                if old.required != row.required {
                    let _ = writeln!(
                        report,
                        "      required: {} -> {} (S2/breaking either direction — an existing request stops being valid, or a required field becomes optional)",
                        old.required, row.required
                    );
                }
                if old.ty != row.ty {
                    let class = if row.ty.contains("null") && !old.ty.contains("null") {
                        "S1/minor (nullability widened — strictly more requests accepted)"
                    } else if old.ty.contains("null") && !row.ty.contains("null") {
                        "S2/breaking (nullability tightened)"
                    } else {
                        "S2/breaking (type changed)"
                    };
                    let _ = writeln!(report, "      type: {} -> {} — {class}", old.ty, row.ty);
                }
                if old.default != row.default {
                    let _ = writeln!(
                        report,
                        "      default: {} -> {} — S2/breaking IF the runtime changed with it, else S0 DEFECT: the advertised and applied defaults must never diverge (policy §5)",
                        old.default, row.default
                    );
                }
                if old.minimum != row.minimum {
                    let _ = writeln!(
                        report,
                        "      minimum: {} -> {} (widening is S1/minor, tightening is S2/breaking)",
                        old.minimum, row.minimum
                    );
                }
                if old.maximum != row.maximum {
                    let _ = writeln!(
                        report,
                        "      maximum: {} -> {} (widening is S1/minor, tightening is S2/breaking)",
                        old.maximum, row.maximum
                    );
                }
            },
            Some(_) => {},
        }
    }
    for (key, row) in pinned {
        if !live.contains_key(key) {
            let _ = writeln!(
                report,
                "  - {}/{}: REMOVED PROPERTY — S2/breaking (policy §3.1). With deny_unknown_fields on every params struct, an old client sending it is now REJECTED, not ignored: {}",
                key.0, key.1, row.ty
            );
        }
    }
    report
}

/// The tool inventory is append-only within a major. A removed or renamed tool
/// is the sharpest break in the whole surface — a client call that used to
/// resolve now does not — so it gets its own test rather than riding along
/// inside the property diff.
#[test]
fn tool_inventory_is_pinned() {
    if record_if_requested() {
        return;
    }
    let live = live_surface();
    let pinned = Surface::parse(&read_fixture());
    let live_names: Vec<&String> = live.tools.iter().map(|(t, _)| t).collect();
    let pinned_names: Vec<&String> = pinned.tools.iter().map(|(t, _)| t).collect();
    if live_names != pinned_names {
        let mut report = String::from("tool inventory moved (policy §3.1):\n");
        for name in &live_names {
            if !pinned_names.contains(name) {
                let _ = writeln!(report, "  + {name}: new tool — S1/minor");
            }
        }
        for name in &pinned_names {
            if !live_names.contains(name) {
                let _ = writeln!(
                    report,
                    "  - {name}: tool REMOVED — S2/breaking, requires a major"
                );
            }
        }
        panic!("{report}");
    }
    assert_eq!(
        live.tools, pinned.tools,
        "a tool's `additionalProperties` moved; relaxing it stops deny_unknown_fields (S2/breaking)"
    );
}

/// The compatibility matrix itself (policy §5).
#[test]
fn advertised_property_matrix_is_pinned() {
    if record_if_requested() {
        return;
    }
    let live = live_surface();
    let pinned = Surface::parse(&read_fixture());
    assert!(
        !live.properties.is_empty(),
        "the derived surface is empty — the fixture would pin nothing"
    );
    if live.properties != pinned.properties {
        let report = diff_rows(&live.properties, &pinned.properties);
        panic!(
            "advertised input schema moved without a decision ({} row(s)):\n{report}\n\
             classify each row per docs/src/mcp-public-surface-policy.md §3.1, then:\n\
             - S0 -> the fixture was stale or the change is drift; fix the code, not the fixture\n\
             - S1/S2 -> scripts/check_mcp_public_surface.sh --record, then bump contract_version",
            live.properties.len() as i64 - pinned.properties.len() as i64
        );
    }
}

/// A nested `$defs` entry's constant set is how an enum variant reaches the
/// wire (`SnapshotFormatParams` is the only one today). Pinning the constants
/// for every tool — not only the seven the #1612 snapshot covers byte-for-byte
/// — is what turns \"a variant was added\" into a failing test instead of a
/// surprise for the client.
#[test]
fn nested_enum_definitions_are_pinned() {
    if record_if_requested() {
        return;
    }
    let live = live_surface();
    let pinned = Surface::parse(&read_fixture());
    assert_eq!(
        live.defs, pinned.defs,
        "a nested schema definition changed: a new $defs entry, or a new/removed enum constant. \
         A new constant is S1/minor (announce it — clients matching exhaustively must add a \
         wildcard); a removed one is S2/breaking."
    );
}

/// The marker must exist and be a positive integer, or the shell guard's
/// "did it move?" comparison has nothing to compare.
#[test]
fn contract_version_is_a_positive_integer() {
    let text = std::fs::read_to_string(VERSION_PATH).unwrap_or_else(|e| {
        panic!(
            "version marker unreadable at {VERSION_PATH}: {e}\n\
             it is the decision point the policy needs; create it with a single positive integer"
        )
    });
    let trimmed = text.trim();
    let parsed: u32 = trimmed.parse().unwrap_or_else(|e| {
        panic!("contract_version marker at {VERSION_PATH} is not an integer: {trimmed:?} ({e})")
    });
    assert!(
        parsed >= 1,
        "contract_version starts at 1 and only ever increases; found {parsed}"
    );
}

/// The exclusion is a real exclusion, not a comment: if the probe ever escapes
/// into the product inventory, this fails and the fixture cannot quietly absorb
/// it either.
#[test]
fn env_gated_probe_is_excluded_from_the_inventory() {
    if record_if_requested() {
        return;
    }
    let pinned = Surface::parse(&read_fixture());
    assert!(
        pinned
            .tools
            .iter()
            .all(|(tool, _)| tool != PANIC_PROBE_TOOL_NAME),
        "the env-gated panic probe ({PANIC_PROBE_TOOL_NAME}) must stay out of the pinned \
         inventory: it is registered only when an operator sets its env var, so pinning it \
         would make the fixture depend on the environment"
    );
    // The same exclusion applies on the live side, so a leaked probe shows up
    // as an inventory diff rather than as a silent pass.
    let live = live_surface();
    assert!(
        live.tools
            .iter()
            .all(|(tool, _)| tool != PANIC_PROBE_TOOL_NAME),
        "the probe reached the live router while un-gated — policy §7 says registration is \
         unconditional for product tools and env-gated only for this probe"
    );
}
