//! Advertised contract vs runtime behavior (issue #1294, NS-04 + NS-02).
//!
//! Both items are the same root: what an MCP client is *told* about a tool is not
//! what the tool *does*. Nothing here is a framework artifact — these strings and
//! schemas are ours.
//!
//! - **NS-04** — `concurrency` advertises `(default: 4)` in its doc comment
//!   (`mcp_server/params.rs:363`) while the handler applies
//!   `ScraperConfig::default().scraper_concurrency`, which is **3**
//!   (`handlers/scraping.rs:255-258` → `domain/config.rs:113`, consumed unclamped at
//!   `application/scraper_service.rs:779`). And because `concurrency` has no CLI
//!   twin, it is outside the OptionsSpec bridge (`schema_bridge.rs:99-108`), so the
//!   existing parity suite never looks at it. The advertised `default` is produced by
//!   exactly the mechanism that already fixes this class of drift —
//!   `DefaultOverride::Set` (#940 F1/F2, "schema truth outranks spec-default
//!   propagation", `schema_bridge.rs:115-120`) — and the bounds come from the
//!   `SetBounds` sub-slice approved for this issue.
//! - **NS-02** — `discover_sitemap` advertises "Auto-discover a website's **sitemap
//!   URL**" (`handlers/scraping.rs:633-637`) but returns the **page URLs inside** the
//!   sitemap (`scraping.rs:655-676`); the real sitemap-URL discoverer
//!   (`sitemap_discovery.rs:513`) is private. The wire payload is deliberately frozen
//!   (`sitemap_discovery.rs:23-24`, `tests/discover_sitemap_wiremock.rs`), so the
//!   description is the half that moves.
//!
//! Run with: `cargo nextest run -p webfang_mcp --features mcp --test mcp_advertised_defaults_test`

#![cfg(feature = "mcp")]

use serde_json::{json, Map, Value};

use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_core::domain::config::ScraperConfig;
use webfang_mcp::mcp_server::params::ScrapeBatchParams;
use webfang_mcp::mcp_server::schema_bridge::{
    default_overrides_for_tool, merged_input_schema, SCRAPE_BATCH_PROPERTIES,
};
use webfang_mcp::mcp_server::McpHandler;

/// Tool whose MCP-only parameter is under contract.
const TOOL: &str = "scrape_batch";

/// The property the issue is about.
const CONCURRENCY: &str = "concurrency";

/// Bounds `ScrapeBatchParams::validate` actually enforces (`params.rs:405-407`).
const CONCURRENCY_MIN: u64 = 1;
const CONCURRENCY_MAX: u64 = 64;

/// The concurrency the tool applies when the client omits the field — read from the
/// same expression the handler uses, never re-typed here, so this test cannot drift
/// into pinning a second copy of the default.
fn runtime_default_concurrency() -> u64 {
    ScraperConfig::default().scraper_concurrency as u64
}

/// scrape_batch's input schema exactly as production renders it: bridge tables plus
/// the tool's advertised-default overrides, through the shared code path
/// (mirrors `merged_with_overrides` in `options_spec_parity_test.rs`).
fn production_properties() -> Map<String, Value> {
    let overrides = default_overrides_for_tool(TOOL);
    let merged = merged_input_schema::<ScrapeBatchParams>(SCRAPE_BATCH_PROPERTIES, &overrides);
    (*merged).clone()
}

/// One advertised property, or a test failure that names the tool.
fn property(name: &str) -> Value {
    let props = production_properties();
    props
        .get(name)
        .unwrap_or_else(|| panic!("{TOOL} must advertise a '{name}' property, got: {props:?}"))
        .clone()
}

/// The `default: <value>` fragment inside a description, when the prose claims one.
///
/// Scans for `default:` and takes the following token up to the first character that
/// cannot be part of a scalar (` `, `,`, `)`, `.`, `;`). Returns a `&str` slice of the
/// description, so no allocation is needed for the comparison.
fn prose_default(description: &str) -> Option<&str> {
    let after = description.split("default:").nth(1)?;
    let token_start = after.len() - after.trim_start().len();
    let rest = &after[token_start..];
    let end = rest
        .find(|c: char| c == ' ' || c == ',' || c == ')' || c == '.' || c == ';')
        .unwrap_or(rest.len());
    let token = &rest[..end];
    (!token.is_empty()).then_some(token)
}

/// How a schema `default` should read when compared against prose: JSON renders
/// booleans and numbers without quotes, which is what descriptions say too.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

// ============================================================================
// NS-04 — the advertised concurrency default
// ============================================================================

/// Control/characterization: the schemars derive alone does NOT advertise a
/// default for `concurrency`.
///
/// This pins *where* the fix belongs — the bridge's `DefaultOverride` table, which
/// is the mechanism the repo already uses for the same bug class on `crawl_site`
/// and `delay_ms` — and it stays green before and after the change.
#[test]
fn derived_schema_alone_advertises_no_concurrency_default() {
    let derived = merged_input_schema::<ScrapeBatchParams>(&[], &[]);
    let prop = derived.get(CONCURRENCY).unwrap_or_else(|| {
        panic!("concurrency must survive the empty-bridge derive, got: {derived:?}")
    });
    assert!(
        prop.get("default").is_none(),
        "an `Option<usize>` field must not gain a machine default from the derive \
         alone; if it does, the override table is the wrong place for the fix: {prop}"
    );
}

/// NS-04 red test: the advertised default must exist and must equal what the tool
/// applies when the field is omitted (3), not the number written in the doc comment
/// (4).
#[test]
fn advertised_concurrency_default_matches_the_runtime_default() {
    let prop = property(CONCURRENCY);
    let advertised = prop.get("default").unwrap_or_else(|| {
        panic!("{CONCURRENCY} must advertise a machine-readable default, got: {prop}")
    });

    assert_eq!(
        advertised,
        &json!(runtime_default_concurrency()),
        "the advertised {CONCURRENCY} default must be the value the tool applies when \
         the field is absent"
    );
}

/// NS-04 red test: prose may not claim a default the schema does not carry.
///
/// Every `scrape_batch` property that states `default: <x>` in its description must
/// also expose `default: <x>` as JSON. Today `concurrency` claims 4 with no machine
/// default (and the wrong number), and `ignore_robots` claims `false` with none —
/// the same lie, one tool, one table. Generalizing this scan beyond `scrape_batch` is
/// a follow-up, not part of this slice.
#[test]
fn every_prose_default_is_also_a_machine_readable_default() {
    let props = production_properties();
    let mut checked = 0usize;
    for (name, prop) in props {
        let Value::Object(prop) = prop else { continue };
        let Some(Value::String(description)) = prop.get("description") else {
            continue;
        };
        let Some(claimed) = prose_default(description) else {
            continue;
        };
        checked += 1;
        let advertised = prop
            .get("default")
            .and_then(scalar_text)
            .unwrap_or_else(|| {
                panic!(
                    "property '{name}' claims `default: {claimed}` in prose but advertises no \
                     machine-readable default: {prop}"
                )
            });
        assert_eq!(
            advertised, claimed,
            "property '{name}' prose default and machine default disagree"
        );
    }
    assert!(
        checked >= 2,
        "the scan found nothing to check ({checked}); the suite would be vacuous"
    );
}

/// NS-04 `SetBounds` sub-slice (approved): the bounds the validator enforces must be
/// discoverable from the schema, not only from a rejected call.
#[test]
fn concurrency_bounds_are_advertised_in_schema() {
    let prop = property(CONCURRENCY);
    assert_eq!(
        prop.get("minimum"),
        Some(&json!(CONCURRENCY_MIN)),
        "{CONCURRENCY} must advertise its lower bound as enforced by validate(): {prop}"
    );
    assert_eq!(
        prop.get("maximum"),
        Some(&json!(CONCURRENCY_MAX)),
        "{CONCURRENCY} must advertise its upper bound as enforced by validate(): {prop}"
    );
}

// ============================================================================
// NS-02 — discover_sitemap promises what it does not return
// ============================================================================

/// Build the handler exactly as the binaries do, so the description under test is
/// the one actually shipped to clients.
async fn shipped_tools() -> Vec<Value> {
    let container = Container::from_config(Config::default())
        .await
        .expect("container creation failed");
    let handler = McpHandler::new(webfang_mcp::mcp_server::McpState::new(container));
    let tools = handler.tool_router.list_all();
    serde_json::to_value(tools)
        .expect("tool list must serialize for inspection")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// NS-02 red test: the advertised description must not promise a sitemap URL.
///
/// The tool returns the pages *inside* the sitemap (`handlers/scraping.rs:655-676`),
/// and that payload is a frozen wire contract, so the string is what has to change.
#[tokio::test]
async fn discover_sitemap_does_not_advertise_a_sitemap_url_it_never_returns() {
    let tools = shipped_tools().await;
    let tool = tools
        .iter()
        .find(|t| t.get("name").and_then(Value::as_str) == Some("discover_sitemap"))
        .expect("discover_sitemap must be registered");
    let description = tool
        .get("description")
        .and_then(Value::as_str)
        .expect("discover_sitemap must ship a description");

    let lowered = description.to_lowercase();
    assert!(
        !lowered.contains("sitemap url"),
        "discover_sitemap advertises a sitemap URL it never returns; describe the page \
         list it actually emits. Current description: {description:?}"
    );
    assert!(
        lowered.contains("sitemap"),
        "the corrected description must still say which source it reads from: {description:?}"
    );
}

/// Companion characterization (green today and after): the tool really is registered
/// with the same name, so the rename option stays off the table and only the wording
/// moved.
#[tokio::test]
async fn discover_sitemap_is_registered_with_its_current_name() {
    let tools = shipped_tools().await;
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .collect();
    assert!(
        names.contains(&"discover_sitemap"),
        "renaming would break the frozen wire contract; the name must stay. Got {names:?}"
    );
}
