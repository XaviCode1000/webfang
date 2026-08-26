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
        CrawlSiteParams, ExportFileParams, GetAccessibilitySnapshotParams,
        ProcessExportPipelineParams, ScrapeBatchParams, ScrapeWithOptionsParams,
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
    /// Per-tool description override for the advertised MCP schema
    /// (issue #948 F6). When `Some`, the bridge uses this string as the
    /// property's `"description"` instead of `spec.help`. Most rows leave
    /// this `None` and fall back to the spec's `description_override`
    /// (or `help` if neither is set).
    pub description_override: Option<&'static str>,
}

const fn prop(name: &'static str, spec: &'static OptionSpec) -> SpecProperty {
    SpecProperty {
        name,
        spec,
        description_override: None,
    }
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
/// The `url` row carries a per-tool description override (issue #948 F6) —
/// the spec's `help` reads "URL to scrape (required unless using a
/// subcommand)" which is CLI-flavored and confusing for LLM consumers of
/// this optional-parameter tool.
pub const PROCESS_EXPORT_PIPELINE_PROPERTIES: &[SpecProperty] = &[
    SpecProperty {
        name: "url",
        spec: &crawler::URL,
        description_override: Some(
            "Optional URL to scrape before exporting. Omit to skip scraping and run the export stage on previously-saved content.",
        ),
    },
    prop("format", &export::EXPORT_FORMAT),
];

/// `scrape_batch`: the `ignore_robots` field overlaps `crawler::GROUP`
/// (issue #948 coverage gap — the tool was registered in WU3 without
/// a bridge table). Other params (`urls`, `concurrency`) are MCP-only
/// and stay on the schemars derive.
pub const SCRAPE_BATCH_PROPERTIES: &[SpecProperty] =
    &[prop("ignore_robots", &crawler::IGNORE_ROBOTS)];

/// `get_accessibility_snapshot`: the `selector` field overlaps
/// `crawler::GROUP` (issue #948 coverage gap). `url`, `interactive_only`,
/// and `format` stay on the schemars derive.
pub const GET_ACCESSIBILITY_SNAPSHOT_PROPERTIES: &[SpecProperty] =
    &[prop("selector", &crawler::SELECTOR)];

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
///
/// Nullability preservation (issue #948 F5): when the derived schema
/// advertises a property as `["<inner>", "null"]` (the schemars form for
/// `Option<T>`) and the spec entry is non-nullable (`nullable: false`),
/// the bridge promotes the spec-rendered `type` to the same
/// `["<inner>", "null"]` shape so the advertised contract matches the
/// serde acceptance. `SpecProperty::spec.nullable = true` upgrades the
/// shape unconditionally; the default behavior preserves the derived
/// `null`-union exactly as the derive emitted it.
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
        let derived_nullable = derived_type_is_nullable(props.get(property.name));
        let mut rendered = property.spec.json_schema();
        // Promote to ["<inner>", "null"] when the derived shape is
        // nullable AND the spec entry is either explicitly nullable or
        // doesn't override nullability. Without this, a `Option<T>` MCP
        // field is advertised as `"type": "boolean"` while serde still
        // accepts `null` — a stricter contract than the runtime
        // acceptance (#948 F5).
        if derived_nullable && !property.spec.nullable {
            promote_spec_to_nullable(&mut rendered);
        }
        // When the spec is explicitly nullable, the derive path becomes
        // irrelevant — the spec is the source of truth.
        if property.spec.nullable {
            promote_spec_to_nullable(&mut rendered);
        }
        // F6: per-tool description override wins over the spec's
        // own override (and over `help`). Lets the bridge tailor
        // descriptions to the MCP consumer without coupling CLI
        // help wording to the LLM-facing surface.
        if let Some(override_) = property
            .description_override
            .or(property.spec.description_override)
        {
            if let Value::Object(map) = &mut rendered {
                map.insert("description".into(), Value::String(override_.to_owned()));
            }
        }
        props.insert(property.name.to_owned(), rendered);
    }
    apply_default_overrides(&mut props, default_overrides);
    merged.insert("properties".into(), Value::Object(props));
    Arc::new(merged)
}

/// True when the derived property advertises `null` in its `type` field
/// — either as `["<inner>", "null"]` (the schemars `Option<T>` form) or
/// the legacy `{"type": "null"}` (none of our params use it, kept for
/// forward-compat). The schemars 0.8 derive renders the `null` member as
/// the STRING `"null"` (not the JSON `null` literal), so we match both
/// representations.
fn derived_type_is_nullable(value: Option<&Value>) -> bool {
    let Some(Value::Object(prop)) = value else {
        return false;
    };
    match prop.get("type") {
        Some(Value::Array(types)) => types.iter().any(|t| match t {
            Value::Null => true,
            Value::String(s) => s == "null",
            _ => false,
        }),
        Some(Value::String(_)) => false,
        _ => false,
    }
}

/// Promote a rendered schema's `type` to the `["<inner>", "null"]` form
/// (no-op when the inner type is missing or already a union). Emits the
/// `null` member as the STRING `"null"` to match the schemars 0.8
/// convention (JSON Schema 2020-12 accepts both forms; the LLM
/// consumers we target already understand the string form).
///
/// Public so the CLI↔MCP parity test (`options_spec_parity_test`) can
/// apply the same nullability promotion to its expected fragment that
/// the bridge applies to the production rendering (#948 F5).
pub fn promote_spec_to_nullable(rendered: &mut Value) {
    let Value::Object(map) = rendered else {
        return;
    };
    match map.get("type") {
        Some(Value::Array(_)) => {}, // already nullable
        Some(Value::String(inner)) => {
            let inner = inner.clone();
            map.insert(
                "type".into(),
                Value::Array(vec![Value::String(inner), Value::String("null".to_owned())]),
            );
        },
        _ => {},
    }
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

fn scrape_batch_input_schema() -> Arc<Map<String, Value>> {
    merged_input_schema::<ScrapeBatchParams>(SCRAPE_BATCH_PROPERTIES, &[])
}

fn get_accessibility_snapshot_input_schema() -> Arc<Map<String, Value>> {
    merged_input_schema::<GetAccessibilitySnapshotParams>(
        GET_ACCESSIBILITY_SNAPSHOT_PROPERTIES,
        &[],
    )
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
    ("scrape_batch", scrape_batch_input_schema as InputSchemaFn),
    (
        "get_accessibility_snapshot",
        get_accessibility_snapshot_input_schema as InputSchemaFn,
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
        // Precondition: the spec entry DOES carry a CLI default (Uint(10),
        // rendered as a JSON number by issue #948 F4) that must be stripped
        // for this tool.
        assert_eq!(crawler::MAX_PAGES.json_schema()["default"], json!(10u64));

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

    /// Proof (issue #948 F5): an `Option<T>` MCP param is advertised as
    /// `["<inner>", "null"]` (the schemars-derived `Option<T>` shape),
    /// not the stricter non-nullable `type: "boolean"` the spec entry's
    /// `json_schema()` would otherwise emit. The bridge preserves the
    /// derive's `null`-union so the advertised contract matches serde's
    /// actual acceptance.
    #[test]
    fn scrape_with_options_advertises_optional_properties_as_nullable() {
        let schema = scrape_with_options_input_schema();
        let props = &schema["properties"];

        // ignore_robots: Option<bool> in the params struct → ["boolean", "null"].
        let ignore_robots = &props["ignore_robots"];
        assert_eq!(
            ignore_robots["type"],
            json!(["boolean", "null"]),
            "ignore_robots (Option<bool>) must advertise a nullable type, got: {ignore_robots}"
        );

        // selector: Option<String> → ["string", "null"].
        let selector = &props["selector"];
        assert_eq!(
            selector["type"],
            json!(["string", "null"]),
            "selector (Option<String>) must advertise a nullable type, got: {selector}"
        );

        // download_images: Option<bool> → ["boolean", "null"].
        let download_images = &props["download_images"];
        assert_eq!(
            download_images["type"],
            json!(["boolean", "null"]),
            "download_images (Option<bool>) must advertise a nullable type"
        );

        // download_documents: Option<bool> → ["boolean", "null"].
        let download_documents = &props["download_documents"];
        assert_eq!(
            download_documents["type"],
            json!(["boolean", "null"]),
            "download_documents (Option<bool>) must advertise a nullable type"
        );

        // Sanity: the derived shape WAS `["<inner>", "null"]` — the
        // bridge is preserving it, not inventing it.
        let derived = raw_derived::<ScrapeWithOptionsParams>();
        assert_eq!(
            derived["properties"]["ignore_robots"]["type"],
            json!(["boolean", "null"])
        );
        assert_eq!(
            derived["properties"]["selector"]["type"],
            json!(["string", "null"])
        );

        // Non-optional fields (e.g. `url`) keep their non-nullable type.
        assert_eq!(props["url"]["type"], json!("string"));
    }

    /// Proof (issue #948 F6): a per-tool `description_override` on a
    /// `SpecProperty` wins over the spec entry's `help` and over the
    /// spec's own `description_override`. The bridge renders the
    /// override as the advertised `"description"` so the LLM-facing
    /// surface is decoupled from the CLI help wording.
    #[test]
    fn process_export_pipeline_url_advertises_per_tool_description_override() {
        let schema = process_export_pipeline_input_schema();
        let url = &schema["properties"]["url"];
        let description = url["description"]
            .as_str()
            .expect("description must be a string");
        assert!(
            !description.contains("required unless using a subcommand"),
            "per-tool override must replace the CLI-flavored help text, got: {description}"
        );
        assert!(
            description.contains("Optional URL"),
            "per-tool override must surface the LLM-facing wording, got: {description}"
        );

        // Sanity: the spec entry's `help` is unchanged (the override
        // lives at the bridge layer, not the SSOT).
        assert_eq!(
            crawler::URL.help,
            "URL to scrape (required unless using a subcommand)"
        );
    }

    /// Proof (issue #948 F6): when neither `SpecProperty::description_override`
    /// nor `OptionSpec::description_override` is set, the bridge falls
    /// back to the spec's `help` text (no behavior change for entries
    /// that haven't been overridden).
    #[test]
    fn crawl_site_url_falls_back_to_spec_help_when_no_override() {
        let schema = crawl_site_input_schema();
        let url = &schema["properties"]["url"];
        assert_eq!(
            url["description"],
            json!(crawler::URL.help),
            "no override → description must equal spec.help"
        );
    }

    /// Proof (issue #948 coverage gap): `scrape_batch` now has a bridge
    /// table that anchors its `ignore_robots` parameter to the spec
    /// entry. Pre-WU5 the field was bridged by nothing — the schemars
    /// derive carried the field with no spec-source bounds/description.
    /// Post-WU5 the bridge renders it through the spec's `json_schema()`.
    #[test]
    fn scrape_batch_ignores_robots_renders_through_spec_entry() {
        let schema = scrape_batch_input_schema();
        let rendered = &schema["properties"]["ignore_robots"];

        // The spec entry's `json_schema()` is the SSOT rendering.
        let expected = crawler::IGNORE_ROBOTS.json_schema();
        // F5: the param is `Option<bool>` so the bridge promotes to
        // `["boolean", "null"]`; the expected is the spec's
        // non-nullable form.
        assert_eq!(
            rendered["type"],
            json!(["boolean", "null"]),
            "ignore_robots (Option<bool>) must advertise a nullable type"
        );
        // All other dimensions (description, default) match the spec.
        assert_eq!(rendered["description"], expected["description"]);
        assert_eq!(rendered["default"], expected["default"]);

        // Sanity: the schemars derive alone advertises ignore_robots
        // WITHOUT the spec's boolean default — bridge is the source of
        // truth.
        let derived = raw_derived::<ScrapeBatchParams>();
        let derived_ignore_robots = &derived["properties"]["ignore_robots"];
        assert_ne!(
            rendered["default"], derived_ignore_robots["default"],
            "bridge must override the schemars derive's default with the spec's"
        );
    }

    /// Proof (issue #948 coverage gap): `get_accessibility_snapshot` now
    /// has a bridge table that anchors its `selector` parameter to the
    /// spec entry. Pre-WU5 the field was bridged by nothing.
    #[test]
    fn get_accessibility_snapshot_selector_renders_through_spec_entry() {
        let schema = get_accessibility_snapshot_input_schema();
        let rendered = &schema["properties"]["selector"];

        let expected = crawler::SELECTOR.json_schema();
        // F5: selector is `Option<String>` so the bridge promotes to
        // `["string", "null"]`.
        assert_eq!(
            rendered["type"],
            json!(["string", "null"]),
            "selector (Option<String>) must advertise a nullable type"
        );
        assert_eq!(rendered["description"], expected["description"]);
        assert_eq!(rendered["default"], expected["default"]);
    }

    /// Proof: the router wiring actually swaps in the bridge schemas for
    /// the two new tools (issue #948 coverage gap). Without the OVERRIDES
    /// entries, the router would emit the raw schemars derive.
    #[test]
    fn build_tool_router_emits_bridge_schemas_for_scrape_batch_and_accessibility() {
        let router = handlers::build_tool_router();

        let route = router
            .map
            .get("scrape_batch")
            .expect("scrape_batch registered");
        let emitted = &route.attr.input_schema["properties"]["ignore_robots"];
        let derived = &raw_derived::<ScrapeBatchParams>()["properties"]["ignore_robots"];
        assert_ne!(
            emitted, derived,
            "scrape_batch router must not emit the raw derive for ignore_robots"
        );

        let route = router
            .map
            .get("get_accessibility_snapshot")
            .expect("get_accessibility_snapshot registered");
        let emitted = &route.attr.input_schema["properties"]["selector"];
        let derived = &raw_derived::<GetAccessibilitySnapshotParams>()["properties"]["selector"];
        assert_ne!(
            emitted, derived,
            "get_accessibility_snapshot router must not emit the raw derive for selector"
        );
    }
}
