//! CLI↔MCP parity table generated from OptionsSpec (#940, ADR-002 slice 4).
//!
//! Proves that EVERY MCP parameter overlapping an OptionsSpec entry advertises
//! exactly the SSOT rendering ([`OptionSpec::json_schema`]) in its tool's
//! bridged input schema — identical type, bounds, and description — and that
//! the schema-bridge tables stay anchored to their declared GROUPs
//! (`crawler::GROUP`, `export::GROUP`). Adding, removing, or renaming a GROUP
//! member forces this table to be reviewed; adding an overlapping MCP param
//! without bridging it fails the completeness check.
//!
//! Modeled on `webfang_core/tests/gate3_config_equivalence_test.rs`: one
//! declarative case table iterated by focused assertions.

use schemars::JsonSchema;
use serde_json::{Map, Value};
use webfang_core::domain::options_spec;
use webfang_core::domain::options_spec::OptionSpec;
use webfang_mcp::mcp_server::params::{
    CrawlSiteParams, ExportFileParams, ProcessExportPipelineParams, ScrapeWithOptionsParams,
};
use webfang_mcp::mcp_server::schema_bridge::{
    merged_input_schema, SpecProperty, CRAWL_SITE_PROPERTIES, EXPORT_FILE_PROPERTIES,
    PROCESS_EXPORT_PIPELINE_PROPERTIES, SCRAPE_WITH_OPTIONS_PROPERTIES,
};

const CRAWLER_GROUP: &[OptionSpec] = options_spec::crawler::GROUP;
const EXPORT_GROUP: &[OptionSpec] = options_spec::export::GROUP;

/// One overlapping MCP parameter: the wire property `wire_name` on `tool`
/// backed by the OptionsSpec entry `spec_id` inside `group`.
struct ParityCase {
    /// Tool whose input schema the bridge overrides.
    tool: &'static str,
    /// MCP wire name of the parameter.
    wire_name: &'static str,
    /// Declared GROUP the spec entry must belong to.
    group: &'static [OptionSpec],
    /// `OptionSpec::id` of the backing entry inside `group`.
    spec_id: &'static str,
}

/// The CLI↔MCP overlap matrix. Every row must hold:
///
/// 1. `spec_id` resolves inside the declared `group`,
/// 2. the resolved entry equals (full value) what the bridge table references,
/// 3. the advertised schema property equals `entry.json_schema()` verbatim.
const PARITY_CASES: &[ParityCase] = &[
    // -- crawl_site ---------------------------------------------------------
    ParityCase {
        tool: "crawl_site",
        wire_name: "url",
        group: CRAWLER_GROUP,
        spec_id: "url",
    },
    ParityCase {
        tool: "crawl_site",
        wire_name: "max_depth",
        group: CRAWLER_GROUP,
        spec_id: "max_depth",
    },
    ParityCase {
        tool: "crawl_site",
        wire_name: "max_pages",
        group: CRAWLER_GROUP,
        spec_id: "max_pages",
    },
    // -- scrape_with_options ------------------------------------------------
    ParityCase {
        tool: "scrape_with_options",
        wire_name: "url",
        group: CRAWLER_GROUP,
        spec_id: "url",
    },
    ParityCase {
        tool: "scrape_with_options",
        wire_name: "max_pages",
        group: CRAWLER_GROUP,
        spec_id: "max_pages",
    },
    ParityCase {
        tool: "scrape_with_options",
        wire_name: "selector",
        group: CRAWLER_GROUP,
        spec_id: "selector",
    },
    ParityCase {
        tool: "scrape_with_options",
        wire_name: "download_images",
        group: CRAWLER_GROUP,
        spec_id: "download_images",
    },
    ParityCase {
        tool: "scrape_with_options",
        wire_name: "download_documents",
        group: CRAWLER_GROUP,
        spec_id: "download_documents",
    },
    ParityCase {
        tool: "scrape_with_options",
        wire_name: "ignore_robots",
        group: CRAWLER_GROUP,
        spec_id: "ignore_robots",
    },
    // -- export_file / process_export_pipeline ------------------------------
    ParityCase {
        tool: "export_file",
        wire_name: "format",
        group: EXPORT_GROUP,
        spec_id: "export_format",
    },
    ParityCase {
        tool: "process_export_pipeline",
        wire_name: "url",
        group: CRAWLER_GROUP,
        spec_id: "url",
    },
    ParityCase {
        tool: "process_export_pipeline",
        wire_name: "format",
        group: EXPORT_GROUP,
        spec_id: "export_format",
    },
];

const TOOL_TABLES: &[ToolTablesErased] = &[
    ToolTablesErased::of::<CrawlSiteParams>("crawl_site", CRAWL_SITE_PROPERTIES),
    ToolTablesErased::of::<ScrapeWithOptionsParams>(
        "scrape_with_options",
        SCRAPE_WITH_OPTIONS_PROPERTIES,
    ),
    ToolTablesErased::of::<ExportFileParams>("export_file", EXPORT_FILE_PROPERTIES),
    ToolTablesErased::of::<ProcessExportPipelineParams>(
        "process_export_pipeline",
        PROCESS_EXPORT_PIPELINE_PROPERTIES,
    ),
];

/// Type-erased handle over [`ToolTable`] so the four typed tools can live in
/// one iterable slice while keeping their generic schema construction.
struct ToolTablesErased {
    tool: &'static str,
    properties: &'static [SpecProperty],
    merged: fn(&'static [SpecProperty]) -> Map<String, Value>,
    derived_properties: fn() -> Vec<String>,
}

impl ToolTablesErased {
    const fn of<P: JsonSchema>(tool: &'static str, properties: &'static [SpecProperty]) -> Self {
        Self {
            tool,
            properties,
            merged: |props| merged_input_schema::<P>(props).as_ref().clone(),
            derived_properties: || derived_property_names::<P>(),
        }
    }

    fn merged_schema(&self) -> Map<String, Value> {
        (self.merged)(self.properties)
    }
}

/// Raw schemars-derived property names of `P`, mirroring what rmcp would
/// advertise before the bridge (root `title`/`description` stripped).
fn derived_property_names<P: JsonSchema>() -> Vec<String> {
    let mut object = match serde_json::to_value(schemars::schema_for!(P)) {
        Ok(Value::Object(map)) => map,
        _ => return Vec::new(),
    };
    object.remove("title");
    object.remove("description");
    match object.get("properties") {
        Some(Value::Object(props)) => props.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

fn resolve_in_group(group: &'static [OptionSpec], spec_id: &str) -> Option<&'static OptionSpec> {
    group.iter().find(|o| o.id == spec_id)
}

/// Every parity row must reference a real GROUP member, and the bridge table
/// must point at THAT EXACT spec entry — compared by full value, since Rust
/// inlines `const` items at each use site (pointer identity is meaningless
/// for them). A bridge drift to a stale duplicate with divergent
/// bounds/description cannot hide here.
#[test]
fn parity_rows_are_anchored_to_declared_groups() {
    for case in PARITY_CASES {
        let resolved = resolve_in_group(case.group, case.spec_id).unwrap_or_else(|| {
            panic!(
                "[{}/{}] spec id {:?} not found in its declared GROUP",
                case.tool, case.wire_name, case.spec_id
            )
        });
        let bridge_entry = TOOL_TABLES
            .iter()
            .find(|t| t.tool == case.tool)
            .unwrap_or_else(|| panic!("tool {} missing from TOOL_TABLES", case.tool))
            .properties
            .iter()
            .find(|p| p.name == case.wire_name)
            .unwrap_or_else(|| {
                panic!(
                    "[{}/{}] missing from the bridge table",
                    case.tool, case.wire_name
                )
            });
        assert_eq!(
            resolved, bridge_entry.spec,
            "[{}/{}] bridge references a different value than GROUP member {:?}",
            case.tool, case.wire_name, case.spec_id
        );
    }
}

/// The advertised schema property equals the SSOT rendering verbatim —
/// type, bounds, and description all travel from OptionsSpec, never from a
/// duplicated literal in MCP code.
#[test]
fn every_overlapping_mcp_param_matches_spec_json_schema() {
    for table in TOOL_TABLES {
        let merged = table.merged_schema();
        let props = merged.get("properties").and_then(Value::as_object);
        let Some(props) = props else {
            panic!("[{}] merged schema has no properties object", table.tool);
        };
        for property in table.properties {
            let expected = property.spec.json_schema();
            assert_eq!(
                props.get(property.name),
                Some(&expected),
                "[{}/{}] advertised schema must be the SSOT rendering",
                table.tool,
                property.name
            );
            // Legibility spot-checks: the three dimensions this parity table
            // exists to pin actually render into the fragment.
            let rendered = &props[property.name];
            assert!(
                rendered.get("description").is_some(),
                "[{}/{}] description missing",
                table.tool,
                property.name
            );
            assert!(
                rendered.get("type").is_some(),
                "[{}/{}] type missing",
                table.tool,
                property.name
            );
            if rendered.get("type") == Some(&Value::String("integer".into())) {
                assert!(
                    rendered.get("minimum").is_some(),
                    "[{}/{}] bounded integer without minimum",
                    table.tool,
                    property.name
                );
            }
        }
    }
}

/// Completeness direction, driven BY the GROUPs: any property of a bridged
/// params struct whose wire name matches a spec id (or a known rename alias)
/// MUST have a parity row. A new overlapping MCP param added without going
/// through the SSOT fails here instead of shipping divergent bounds.
#[test]
fn no_group_overlapping_param_escapes_the_parity_table() {
    let mut spec_ids: Vec<&str> = CRAWLER_GROUP
        .iter()
        .chain(EXPORT_GROUP.iter())
        .map(|o| o.id)
        .collect();
    // Known wire-name renames: MCP `format` carries `export::EXPORT_FORMAT`.
    spec_ids.push("export_format");

    for table in TOOL_TABLES {
        for name in (table.derived_properties)() {
            let overlaps = spec_ids.contains(&name.as_str());
            assert!(
                !overlaps || table.properties.iter().any(|p| p.name == name),
                "[{}/{}] derives a GROUP-overlapping property with NO parity row",
                table.tool,
                name
            );
        }
    }
}
