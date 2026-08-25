//! Schema bridge — MCP tool input schemas derived from the OptionsSpec SSOT
//! (ADR-002 slice 4, #940).
//!
//! rmcp generates each tool's advertised `inputSchema` from the schemars
//! derive on its parameter struct, which duplicates bounds and descriptions
//! that already live (authoritatively) in
//! [`webfang_core::domain::options_spec`]. This module overrides the emitted
//! schema for tools whose parameters overlap a spec group: spec-backed
//! properties are rendered through [`OptionSpec::json_schema`] while MCP-only
//! properties keep their schemars-derived shape untouched. On top of the spec
//! rendering, per-tool *advertised-default overrides* ([`DefaultOverride`])
//! keep the schema TRUTHFUL about runtime: where a handler applies a different
//! default than the CLI/spec one, the bridge advertises the runtime-effective
//! value (or no default at all when the engine decides) (#940 F1/F2).
//!
//! Layering: this is a one-way seam. The bridge imports
//! `webfang_core::domain::options_spec` only; the core never learns about
//! MCP types.

use std::sync::Arc;

use rmcp::handler::server::tool::ToolRouter;
use schemars::JsonSchema;
use serde_json::{json, Map, Value};
use webfang_core::domain::options_spec::{crawler, export, OptionSpec};

use crate::mcp_server::{
    handlers,
    params::{
        CrawlSiteParams, ExportFileParams, ProcessExportPipelineParams, ScrapeWithOptionsParams,
    },
};

/// One spec-backed property of an overridden tool input schema.
#[derive(Debug, Clone, Copy)]
pub struct SpecProperty {
    /// MCP wire name of the property. May differ from [`OptionSpec::id`]
    /// when the tool's parameter name predates the spec identifier.
    pub name: &'static str,
    /// OptionsSpec entry describing the property's type, bounds, default,
    /// and description.
    pub spec: &'static OptionSpec,
}

const fn prop(name: &'static str, spec: &'static OptionSpec) -> SpecProperty {
    SpecProperty { name, spec }
}

/// `crawl_site`: every parameter overlaps the crawler spec group.
pub const CRAWL_SITE_PROPERTIES: &[SpecProperty] = &[
    prop("url", &crawler::URL),
    prop("max_depth", &crawler::MAX_DEPTH),
    prop("max_pages", &crawler::MAX_PAGES),
];

/// `scrape_with_options`: every parameter overlaps the crawler spec group.
pub const SCRAPE_WITH_OPTIONS_PROPERTIES: &[SpecProperty] = &[
    prop("url", &crawler::URL),
    prop("max_pages", &crawler::MAX_PAGES),
    prop("selector", &crawler::SELECTOR),
    prop("download_images", &crawler::DOWNLOAD_IMAGES),
    prop("download_documents", &crawler::DOWNLOAD_DOCUMENTS),
    prop("ignore_robots", &crawler::IGNORE_ROBOTS),
];

/// `export_file`: only `format` overlaps (`export::EXPORT_FORMAT`; wire name
/// differs from the spec id `export_format`). Path/blob params are MCP-only.
pub const EXPORT_FILE_PROPERTIES: &[SpecProperty] = &[prop("format", &export::EXPORT_FORMAT)];

/// `process_export_pipeline`: `url` and `format` overlap their spec entries.
pub const PROCESS_EXPORT_PIPELINE_PROPERTIES: &[SpecProperty] = &[
    prop("url", &crawler::URL),
    prop("format", &export::EXPORT_FORMAT),
];

/// One advertised-default override, applied AFTER spec derivation (#940 F1/F2).
///
/// The OptionsSpec records CLI defaults; a few MCP handlers apply different
/// runtime defaults. Schema truth outranks spec-default propagation: these
/// overrides adjust what the bridged schema advertises so it matches what the
/// tool actually does.
#[derive(Debug, Clone)]
pub enum DefaultOverride {
    /// Advertise this JSON value as the property's `"default"`, replacing the
    /// spec-derived default. Used to advertise the RUNTIME-effective default.
    Set(Value),
    /// Advertise NO default: the tool forwards the parameter's absence and the
    /// engine decides at runtime — absence is the honest advertisement.
    Unset,
}

/// Per-tool advertised-default overrides: `(property name, override)` pairs.
pub type DefaultOverrides = [(&'static str, DefaultOverride)];

/// Runtime-effective advertised-default overrides for one tool's bridged
/// schema, keyed by registered tool name; unknown tools yield an empty set.
/// THE single source of truth for this mapping — both the schema wrappers and
/// the parity tests derive their expectations from here.
#[must_use]
pub fn default_overrides_for_tool(tool: &str) -> Vec<(&'static str, DefaultOverride)> {
    match tool {
        // `crawl_site`: the handler applies `unwrap_or(3)` / `unwrap_or(100)`
        // (handlers/scraping.rs), which differ from the CLI/spec defaults
        // (2/10). Advertise the values the tool actually applies.
        "crawl_site" => vec![
            (
                "max_depth",
                DefaultOverride::Set(json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_DEPTH)),
            ),
            (
                "max_pages",
                DefaultOverride::Set(json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_PAGES)),
            ),
        ],
        // `scrape_with_options`: an absent `max_pages` is forwarded as `None`,
        // leaving the decision to the engine — advertise no default at all.
        "scrape_with_options" => vec![("max_pages", DefaultOverride::Unset)],
        _ => Vec::new(),
    }
}

/// Apply advertised-default overrides to a rendered-properties map.
///
/// Public so tests derive expected fragments through the same code path as
/// production. An override naming an unknown property is ignored (with a
/// warning) rather than fabricating a property.
pub fn apply_default_overrides(props: &mut Map<String, Value>, overrides: &DefaultOverrides) {
    for (name, override_) in overrides {
        let Some(Value::Object(prop)) = props.get_mut(*name) else {
            tracing::warn!(
                property = *name,
                "default override targets unknown property"
            );
            continue;
        };
        match override_ {
            DefaultOverride::Set(value) => {
                prop.insert("default".into(), value.clone());
            },
            DefaultOverride::Unset => {
                prop.remove("default");
            },
        }
    }
}

/// Build the merged input schema for one tool.
///
/// Starts from the schemars-derived schema of the parameter struct `P`
/// (preserving `required`, `additionalProperties`, and every MCP-only
/// property exactly as rmcp would emit it today), then replaces each
/// spec-covered property with its [`OptionSpec::json_schema`] rendering,
/// finally applying `default_overrides` on top.
#[must_use]
pub fn merged_input_schema<P: JsonSchema>(
    properties: &[SpecProperty],
    default_overrides: &DefaultOverrides,
) -> Arc<Map<String, Value>> {
    let mut merged = derived_root::<P>();
    let mut props = match merged.remove("properties") {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    for property in properties {
        props.insert(property.name.to_owned(), property.spec.json_schema());
    }
    apply_default_overrides(&mut props, default_overrides);
    merged.insert("properties".into(), Value::Object(props));
    Arc::new(merged)
}

/// Schemars-derived root object for `P`, mirroring rmcp 1.8's
/// `validate_and_strip` normalization exactly: top-level `title` and
/// `description` removed; everything else (including root `$schema`) kept.
fn derived_root<P: JsonSchema>() -> Map<String, Value> {
    let Ok(value) = serde_json::to_value(schemars::schema_for!(P)) else {
        // Unreachable for schemars-generated schemas (plain JSON types
        // only); degrade to an empty root rather than panic the server.
        tracing::error!("schemars schema failed to serialize; emitting empty input schema");
        return Map::new();
    };
    let mut object = match value {
        Value::Object(map) => map,
        _ => return Map::new(),
    };
    object.remove("title");
    object.remove("description");
    object
}

fn crawl_site_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("crawl_site");
    merged_input_schema::<CrawlSiteParams>(CRAWL_SITE_PROPERTIES, &overrides)
}

fn scrape_with_options_input_schema() -> Arc<Map<String, Value>> {
    let overrides = default_overrides_for_tool("scrape_with_options");
    merged_input_schema::<ScrapeWithOptionsParams>(SCRAPE_WITH_OPTIONS_PROPERTIES, &overrides)
}

fn export_file_input_schema() -> Arc<Map<String, Value>> {
    merged_input_schema::<ExportFileParams>(EXPORT_FILE_PROPERTIES, &[])
}

fn process_export_pipeline_input_schema() -> Arc<Map<String, Value>> {
    merged_input_schema::<ProcessExportPipelineParams>(PROCESS_EXPORT_PIPELINE_PROPERTIES, &[])
}

type InputSchemaFn = fn() -> Arc<Map<String, Value>>;

/// Tools whose advertised input schema is overridden by the bridge.
///
/// Every entry names a registered tool and the bridge function producing its
/// merged schema. A missing route (tool renamed without updating this table)
/// logs a warning instead of failing the router build.
const OVERRIDES: &[(&str, InputSchemaFn)] = &[
    ("crawl_site", crawl_site_input_schema as InputSchemaFn),
    (
        "scrape_with_options",
        scrape_with_options_input_schema as InputSchemaFn,
    ),
    ("export_file", export_file_input_schema as InputSchemaFn),
    (
        "process_export_pipeline",
        process_export_pipeline_input_schema as InputSchemaFn,
    ),
];

/// Replace the advertised `inputSchema` of every overridden tool in `router`.
pub fn apply_overrides(router: &mut ToolRouter<handlers::McpHandler>) {
    for (tool_name, schema_fn) in OVERRIDES {
        match router.map.get_mut(*tool_name) {
            Some(route) => route.attr.input_schema = schema_fn(),
            None => tracing::warn!(
                tool = *tool_name,
                "schema_bridge override target not found in tool router"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};
    use webfang_core::domain::options_spec::export;

    use super::*;

    /// Raw schemars rendering of a params struct exactly as rmcp advertises
    /// it BEFORE the bridge (root `$schema` retained).
    fn raw_derived<P: JsonSchema>() -> Value {
        let mut object = match serde_json::to_value(schemars::schema_for!(P)) {
            Ok(Value::Object(map)) => map,
            _ => Map::new(),
        };
        // rmcp strips the struct doc-comment from the advertised schema.
        object.remove("title");
        object.remove("description");
        Value::Object(object)
    }

    fn as_value(schema: &Map<String, Value>) -> Value {
        Value::Object(schema.clone())
    }

    /// Proof: `export_file`'s advertised `format` property is rendered
    /// byte-consistently from `export::EXPORT_FORMAT`, not from the schemars
    /// derive.
    #[test]
    fn export_file_advertises_spec_enum_byte_consistently() {
        let schema = export_file_input_schema();
        assert_eq!(
            schema["properties"]["format"],
            export::EXPORT_FORMAT.json_schema(),
            "format must be the SSOT rendering (enum variants, default, help text)"
        );
        // Override-path proof: the pre-bridge derive had a different shape
        // (no closed enum / spec default), so equality above cannot be an
        // accident of the derive.
        let derived = raw_derived::<ExportFileParams>();
        assert_ne!(
            derived["properties"]["format"],
            schema["properties"]["format"]
        );
    }

    /// Proof (#940 F1): `crawl_site` advertises the RUNTIME-effective
    /// defaults — exactly the handler's `unwrap_or` constants — not the
    /// CLI/spec defaults (2/10), so advertised schema and runtime cannot
    /// drift apart.
    #[test]
    fn crawl_site_advertises_runtime_effective_defaults() {
        let schema = crawl_site_input_schema();
        assert_eq!(
            schema["properties"]["max_depth"]["default"],
            json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_DEPTH)
        );
        assert_eq!(
            schema["properties"]["max_pages"]["default"],
            json!(handlers::scraping::CRAWL_SITE_DEFAULT_MAX_PAGES)
        );
        // The override path actually fired: the fragments differ from the
        // raw spec renderings (which carry the CLI defaults "2"/"10").
        assert_ne!(
            schema["properties"]["max_depth"],
            crawler::MAX_DEPTH.json_schema()
        );
        assert_ne!(
            schema["properties"]["max_pages"],
            crawler::MAX_PAGES.json_schema()
        );
    }

    /// Proof (#940 F2): `scrape_with_options` advertises NO default for
    /// `max_pages` — the handler forwards absence as `None` and the engine
    /// decides, so silence is the honest advertisement.
    #[test]
    fn scrape_with_options_advertises_no_max_pages_default() {
        // Precondition: the spec entry DOES carry a CLI default ("10")
        // that must be stripped for this tool.
        assert_eq!(crawler::MAX_PAGES.json_schema()["default"], json!("10"));

        let schema = scrape_with_options_input_schema();
        assert_eq!(
            crawler::MAX_PAGES.json_schema()["maximum"],
            json!(100_000u64)
        );
        assert_eq!(
            schema["properties"]["max_pages"]["maximum"],
            json!(100_000u64),
            "bounds still travel from the spec"
        );
        assert!(
            schema["properties"]["max_pages"].get("default").is_none(),
            "engine-decided max_pages must not advertise a default"
        );
    }

    /// Proof: numeric bounds travel from `NumericPolicy` into the advertised
    /// schema (the schemars derive on these fields carries no cap).
    #[test]
    fn crawl_site_advertises_numeric_policy_caps() {
        let schema = crawl_site_input_schema();
        assert_eq!(
            schema["properties"]["max_pages"]["maximum"],
            json!(100_000u64)
        );
        assert_eq!(schema["properties"]["max_pages"]["minimum"], json!(1u64));
        assert_eq!(schema["properties"]["max_depth"]["maximum"], json!(10u64));

        // The un-overridden derive advertises max_pages WITHOUT any bound:
        let derived = &raw_derived::<CrawlSiteParams>()["properties"]["max_pages"];
        assert!(derived.get("maximum").is_none());
    }

    /// Proof: the router wiring actually swaps in the bridge schema — the
    /// emitted `inputSchema` equals the bridge output and differs from the
    /// raw derive for a spec-covered property.
    #[test]
    fn build_tool_router_emits_bridge_schemas() {
        let router = handlers::build_tool_router();

        let route = router.map.get("crawl_site").expect("crawl_site registered");
        assert_eq!(
            Value::Object(route.attr.input_schema.as_ref().clone()),
            as_value(&crawl_site_input_schema()),
        );
        let emitted = &route.attr.input_schema["properties"]["max_pages"];
        let derived = &raw_derived::<CrawlSiteParams>()["properties"]["max_pages"];
        assert_ne!(emitted, derived, "router must not emit the raw derive");

        let route = router
            .map
            .get("process_export_pipeline")
            .expect("process_export_pipeline registered");
        assert_eq!(
            Value::Object(route.attr.input_schema.as_ref().clone()),
            as_value(&process_export_pipeline_input_schema()),
        );

        // Non-overridden tools keep their derive untouched.
        let route = router.map.get("scrape_url").expect("scrape_url registered");
        assert_eq!(
            Value::Object(route.attr.input_schema.as_ref().clone()),
            raw_derived::<crate::mcp_server::params::ScrapeUrlParams>(),
        );
    }
}
