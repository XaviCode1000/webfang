//! CLI↔MCP parity table generated from OptionsSpec (#940, ADR-002 slice 4).
//!
//! Proves that EVERY MCP parameter overlapping an OptionsSpec entry advertises
//! exactly the SSOT rendering ([`OptionSpec::json_schema`]) in its tool's
//! bridged input schema — identical type, bounds, and description — EXCEPT
//! where a runtime-effective advertised-default override applies (#940 F1/F2:
//! schema truth over spec-default propagation). It also proves the bridge
//! tables stay anchored to their declared GROUPs (`crawler::GROUP`,
//! `export::GROUP`). Adding, removing, or renaming a GROUP member forces this
//! table to be reviewed; adding an overlapping MCP param without bridging it
//! fails the completeness check.
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
    apply_default_overrides, default_overrides_for_tool, merged_input_schema, promote_spec_to_nullable,
    SpecProperty, CRAWL_SITE_PROPERTIES, EXPORT_FILE_PROPERTIES, PROCESS_EXPORT_PIPELINE_PROPERTIES,
    SCRAPE_WITH_OPTIONS_PROPERTIES,
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
/// 3. the advertised schema property equals `entry.json_schema()` verbatim,
///    except where [`default_overrides_for_tool`] adjusts the advertised
///    default (expectations are derived through the SAME override code path
///    as production).
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
    merged: fn(&'static [SpecProperty], &'static str) -> Map<String, Value>,
    derived_properties: fn() -> Vec<String>,
    /// Raw schemars-derived properties keyed by name — used by the parity
    /// test to apply the same nullability promotion the bridge does
    /// (issue #948 F5).
    derived: fn() -> Map<String, Value>,
}

impl ToolTablesErased {
    const fn of<P: JsonSchema>(tool: &'static str, properties: &'static [SpecProperty]) -> Self {
        Self {
            tool,
            properties,
            merged: |props, tool| merged_with_overrides::<P>(props, tool),
            derived_properties: || derived_property_names::<P>(),
            derived: || derived_properties::<P>(),
        }
    }

    fn merged_schema(&self) -> Map<String, Value> {
        (self.merged)(self.properties, self.tool)
    }

    fn derived_schema(&self) -> Map<String, Value> {
        (self.derived)()
    }
}

/// Production-equivalent merge: spec rendering PLUS the tool's
/// advertised-default overrides, via one shared code path.
fn merged_with_overrides<P: JsonSchema>(
    properties: &'static [SpecProperty],
    tool: &'static str,
) -> Map<String, Value> {
    let overrides = default_overrides_for_tool(tool);
    merged_input_schema::<P>(properties, &overrides)
        .as_ref()
        .clone()
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

/// Raw schemars-derived properties of `P` keyed by name — used by the
/// parity test to apply the same nullability promotion the bridge does
/// (issue #948 F5).
fn derived_properties<P: JsonSchema>() -> Map<String, Value> {
    let mut object = match serde_json::to_value(schemars::schema_for!(P)) {
        Ok(Value::Object(map)) => map,
        _ => return Map::new(),
    };
    object.remove("title");
    object.remove("description");
    match object.remove("properties") {
        Some(Value::Object(props)) => props,
        _ => Map::new(),
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
/// duplicated literal in MCP code — except where a runtime-effective
/// default override applies; overridden expectations flow through the
/// same [`apply_default_overrides`] path as production.
#[test]
fn every_overlapping_mcp_param_matches_spec_json_schema() {
    for table in TOOL_TABLES {
        let merged = table.merged_schema();
        let props = merged.get("properties").and_then(Value::as_object);
        let Some(props) = props else {
            panic!("[{}] merged schema has no properties object", table.tool);
        };
        for property in table.properties {
            // Expected fragment = SSOT rendering + the tool's overrides,
            // applied through the production code path (not re-derived).
            let mut expected_holder = Map::new();
            let mut expected_value = property.spec.json_schema();
            // Issue #948 F5: if the derived shape for this property is
            // nullable (e.g. `Option<T>` in the params struct), the bridge
            // promotes the spec-rendered `type` to `["<inner>", "null"]`.
            // Apply the same promotion to the expected so the parity
            // comparison reflects the production rendering, not a stricter
            // pre-F5 shape.
            let derived = table.derived_schema();
            if let Some(derived_prop) = derived.get(property.name) {
                let derived_nullable = match derived_prop.get("type") {
                    Some(Value::Array(types)) => types.iter().any(|t| match t {
                        Value::Null => true,
                        Value::String(s) => s == "null",
                        _ => false,
                    }),
                    _ => false,
                };
                if derived_nullable && !property.spec.nullable {
                    promote_spec_to_nullable(&mut expected_value);
                }
                if property.spec.nullable {
                    promote_spec_to_nullable(&mut expected_value);
                }
            }
            // Issue #948 F6: per-tool description override wins over the
            // spec's `help` (and over the spec's own
            // `description_override`). Mirror the bridge's precedence
            // here so the parity comparison reflects the production
            // rendering.
            if let Some(override_) = property
                .description_override
                .or(property.spec.description_override)
            {
                if let Value::Object(map) = &mut expected_value {
                    map.insert(
                        "description".into(),
                        Value::String(override_.to_owned()),
                    );
                }
            }
            expected_holder.insert(property.name.to_owned(), expected_value);
            let overrides = default_overrides_for_tool(table.tool);
            apply_default_overrides(&mut expected_holder, &overrides);
            let expected = expected_holder
                .remove(property.name)
                .expect("override target must be the property itself");
            assert_eq!(
                props.get(property.name),
                Some(&expected),
                "[{}/{}] advertised schema must be the SSOT rendering (+ overrides)",
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

/// Runtime-effective advertised defaults (#940 F1/F2): `crawl_site`
/// advertises EXACTLY the handler's `unwrap_or` constants (3/100), and
/// `scrape_with_options` advertises NO default for `max_pages` (engine
/// decides). Both expectations are derived from the handler constants, so a
/// future runtime change without a matching bridge change fails here.
#[test]
fn overridden_defaults_advertise_runtime_effective_values() {
    use webfang_mcp::mcp_server::handlers::scraping::{
        CRAWL_SITE_DEFAULT_MAX_DEPTH, CRAWL_SITE_DEFAULT_MAX_PAGES,
    };

    let crawl = merged_with_overrides::<CrawlSiteParams>(CRAWL_SITE_PROPERTIES, "crawl_site");
    assert_eq!(
        crawl["properties"]["max_depth"]["default"],
        Value::from(CRAWL_SITE_DEFAULT_MAX_DEPTH),
        "crawl_site max_depth must advertise the handler's unwrap_or constant"
    );
    assert_eq!(
        crawl["properties"]["max_pages"]["default"],
        Value::from(CRAWL_SITE_DEFAULT_MAX_PAGES),
        "crawl_site max_pages must advertise the handler's unwrap_or constant"
    );

    let scrape = merged_with_overrides::<ScrapeWithOptionsParams>(
        SCRAPE_WITH_OPTIONS_PROPERTIES,
        "scrape_with_options",
    );
    assert!(
        scrape["properties"]["max_pages"].get("default").is_none(),
        "engine-decided max_pages must not advertise any default"
    );

    // Every other crawl_site property keeps its spec rendering untouched.
    let mut expected_url = Map::new();
    expected_url.insert("url".to_owned(), options_spec::crawler::URL.json_schema());
    apply_default_overrides(&mut expected_url, &default_overrides_for_tool("crawl_site"));
    assert_eq!(
        crawl.get("properties").and_then(|p| p.get("url")),
        expected_url.get("url"),
        "non-overridden properties stay verbatim SSOT renderings"
    );
}
