//! SD-07 — the regression fixture for the **advertised** MCP input schemas.
//!
//! Every other finding in this issue class is only *provable* because the
//! generated schemas are pinned here. Without this fixture, "the change did
//! not alter the advertised contract" is an assertion, not evidence: the
//! bridge is a one-way seam over a schemars derive, and both move silently.
//!
//! Two things are captured:
//!
//! 1. **The seven bridged schemas, byte-for-byte, as the router actually
//!    emits them.** The input is [`build_tool_router`] — the production
//!    assembly path (tool derives + [`apply_overrides`]), not a re-derivation
//!    inside the test. A snapshot of a *re-derived* schema would pin the test's
//!    own idea of the schema; a snapshot of the router's `inputSchema` pins
//!    what a client is actually served.
//!
//! 2. **The rmcp 1.8.0 normalization facts**, which SD-07 recorded as
//!    unverified ("the bridge preserves raw `root`, `$schema` and
//!    `definitions`, and rmcp 1.8.0 behavior was never read"). They are now
//!    *read* from rmcp 1.8.0's `handler/server/common.rs` and asserted
//!    empirically against the emitted schema rather than assumed:
//!    - rmcp's `validate_and_strip` removes ONLY root `title` and
//!      `description`; it does **not** touch `$schema`, `$defs` or
//!      `properties`.
//!    - rmcp requires a root `"type": "object"` and errors otherwise, so
//!      every bridged tool must carry it.
//!
//! Scope note: this fixture pins the *schema*, not the tool `description`
//! strings. Descriptions are prose and are asserted where they carry a
//! behavioural claim (see `mcp_schema_compat_test`).

use serde_json::Value;
use webfang_mcp::mcp_server::handlers::build_tool_router;

/// The seven tools whose advertised schema the bridge overrides
/// (`schema_bridge::OVERRIDES`). Each row is snapshot-verbatim, so a
/// property, bound, default, nullability union or description that moves is
/// a failing diff.
const BRIDGED_TOOLS: &[&str] = &[
    "crawl_site",
    "crawl_with_sitemap",
    "scrape_with_options",
    "export_file",
    "process_export_pipeline",
    "scrape_batch",
    "get_accessibility_snapshot",
];

/// One non-overridden tool, pinned as the control for the same fixture.
/// It proves the snapshots are specific: the bridged tools differ from a
/// plain schemars derive, so a snapshot that "matched everything" would be
/// evidence of a fixture that pins nothing.
const UNBRIDGED_CONTROL_TOOL: &str = "scrape_url";

fn advertised(tool: &str) -> Value {
    let router = build_tool_router();
    let route = router
        .map
        .get(tool)
        .unwrap_or_else(|| panic!("tool {tool} must be registered"));
    Value::Object(route.attr.input_schema.as_ref().clone())
}

/// The advertised `inputSchema` as a stable, diff-friendly string.
///
/// `serde_json`'s `Map` preserves insertion order unless the `preserve_order`
/// feature is off, in which case it is a `BTreeMap` — either way the bytes are
/// deterministic for a given schema, and a property/bound/description move
/// shows up as a line diff rather than a reformat.
fn advertised_pretty(tool: &str) -> String {
    serde_json::to_string_pretty(&advertised(tool)).expect("schema must serialize")
}

/// The advertised `inputSchema` of every bridged tool, one snapshot per tool.
#[test]
fn bridged_input_schemas_are_pinned() {
    for tool in BRIDGED_TOOLS {
        insta::assert_snapshot!(format!("bridged_{tool}"), advertised_pretty(tool));
    }
}

/// The non-overridden control: its advertised schema is the raw schemars
/// derive, untouched by the bridge.
#[test]
fn unbridged_control_schema_is_pinned() {
    insta::assert_snapshot!(
        format!("unbridged_{UNBRIDGED_CONTROL_TOOL}"),
        advertised_pretty(UNBRIDGED_CONTROL_TOOL)
    );
}

/// The one bridged tool whose parameters carry no [`McpUrl`], so its
/// generated schema is the only one with no nested definitions at all. It is
/// the negative case for the `$defs` assertion below.
const BRIDGED_TOOL_WITHOUT_URLS: &str = "export_file";

/// The exact nested definitions each bridged tool serves, in the order the
/// generator emits them. Every params struct that names a non-primitive type
/// contributes one entry; a row that grows or shrinks is a wire-contract
/// change and must come through review, not through a re-recorded snapshot.
const EXPECTED_DEFS: &[(&str, &[&str])] = &[
    ("crawl_site", &["McpUrl"]),
    ("crawl_with_sitemap", &["McpUrl"]),
    ("scrape_with_options", &["McpUrl"]),
    ("export_file", &[]),
    ("process_export_pipeline", &["McpUrl"]),
    ("scrape_batch", &["McpUrl"]),
    // The only tool with a second named type: the `format` enum.
    (
        "get_accessibility_snapshot",
        &["McpUrl", "SnapshotFormatParams"],
    ),
];

/// SD-07, resolved empirically rather than by reading the audit: what rmcp
/// 1.8.0's `validate_and_strip` does and does not remove, and where the
/// nested definitions actually live.
///
/// The audit recorded this finding as unverified and named the draft-07
/// `definitions` keyword. It is read now from rmcp 1.8.0's
/// `handler/server/common.rs` and asserted against the *emitted* schema, so
/// the two claims that were guesses become observations:
///
/// - `validate_and_strip` removes ONLY root `title` and `description`. Root
///   `$schema`, `$defs`, `properties`, `required` and `additionalProperties`
///   all reach the client untouched.
/// - rmcp generates with `SchemaSettings::draft2020_12()`, so nested
///   definitions land under **`$defs`**, never `definitions`. The audit's
///   keyword is the draft-07 one and does not appear in any served schema.
///
/// A future change to the generator settings, or a new named nested type,
/// fails here instead of silently changing the wire contract.
#[test]
fn rmcp_normalization_is_pinned() {
    for tool in BRIDGED_TOOLS {
        let schema = advertised(tool);

        // rmcp rejects any inputSchema whose root type is not "object"
        // (`validate_and_strip`, rmcp 1.8.0). Every bridged root must carry
        // it or the router would fail to build.
        assert_eq!(
            schema["type"],
            Value::String("object".into()),
            "{tool}: root type must be \"object\" or rmcp refuses the schema"
        );

        // ... and the ONLY two keywords it strips are root `title` and
        // `description`. Their absence is the pinned observable of that.
        assert!(
            schema.get("title").is_none(),
            "{tool}: rmcp strips root title; it must not reappear"
        );
        assert!(
            schema.get("description").is_none(),
            "{tool}: rmcp strips root description; it must not reappear"
        );

        // rmcp leaves `$schema` in place — the bridge's `derived_root` comment
        // asserts it mirrors `validate_and_strip`; this is the assertion that
        // the mirror holds, and it pins the dialect rmcp 1.8.0 actually emits.
        assert_eq!(
            schema["$schema"],
            Value::String("https://json-schema.org/draft/2020-12/schema".into()),
            "{tool}: rmcp 1.8.0 does not strip root `$schema`, and the dialect is 2020-12"
        );

        // The draft-07 `definitions` keyword the audit named never appears:
        // the 2020-12 generator writes `$defs` instead. Asserted on every
        // tool so a settings downgrade to draft-07 cannot pass review.
        assert!(
            schema.get("definitions").is_none(),
            "{tool}: no draft-07 `definitions` — the 2020-12 generator writes `$defs`"
        );

        // `$defs` is present exactly when the params carry a named type, and
        // then carries exactly the types the table declares — no more, no
        // fewer. A new named type changes the served contract and must come
        // through review.
        let expected = EXPECTED_DEFS
            .iter()
            .find(|(name, _)| *name == *tool)
            .map(|(_, defs)| *defs)
            .unwrap_or_else(|| panic!("{tool}: missing an EXPECTED_DEFS row"));
        let names: Vec<&str> = schema
            .get("$defs")
            .and_then(Value::as_object)
            .map(|defs| defs.keys().map(String::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            names, expected,
            "{tool}: `$defs` changed; a new named type changes the served \
             contract and needs review"
        );

        if expected.is_empty() {
            assert!(
                *tool == BRIDGED_TOOL_WITHOUT_URLS,
                "{tool}: only the URL-free tool may serve no nested definitions"
            );
        }

        // Every object schema advertises its properties; `required` is where
        // the export-format drift of SD-01 becomes visible and is pinned by
        // the per-tool snapshots above.
        assert!(
            schema.get("properties").is_some(),
            "{tool}: every object schema advertises properties"
        );
    }
}

/// The control tool shares the same normalization root, so the normalization
/// assertions above are properties of rmcp + schemars, not of the bridge.
#[test]
fn unbridged_control_shares_the_same_root_normalization() {
    let schema = advertised(UNBRIDGED_CONTROL_TOOL);
    assert_eq!(schema["type"], Value::String("object".into()));
    assert!(schema.get("title").is_none());
    assert!(schema.get("description").is_none());
    assert_eq!(
        schema["$schema"],
        Value::String("https://json-schema.org/draft/2020-12/schema".into())
    );
}
