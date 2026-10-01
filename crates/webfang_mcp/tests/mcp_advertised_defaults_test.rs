//! Advertised contract vs runtime behavior (issue #1294, NS-04 + NS-02).
//!
//! Both items are the same root: what an MCP client is *told* about a tool is not
//! what the tool *does*. Nothing here is a framework artifact — these strings and
//! schemas are ours.
//!
//! - **NS-04** — `concurrency` advertised `(default: 4)` in its doc comment
//!   (`mcp_server/params.rs:363`) while the handler applied
//!   `ScraperConfig::default().scraper_concurrency`, which is **3**
//!   (`handlers/scraping.rs:255-258` → `domain/config.rs:113`, consumed unclamped at
//!   `application/scraper_service.rs:779`), and the rendered schema carried neither a
//!   `default` nor its real bounds (`minimum: 0`, a value the validator rejects).
//!   Because `concurrency` has no CLI twin it is outside the OptionsSpec bridge
//!   (`schema_bridge.rs:99-108`), so the parity suite never looked at it. The fix uses
//!   the mechanism that already handles this class of drift — `DefaultOverride::Set`
//!   (#940 F1/F2, "schema truth outranks spec-default propagation",
//!   `schema_bridge.rs:115-120`) — plus the `SetBounds` variant this issue adds.
//! - **NS-02** — `discover_sitemap` advertises "Auto-discover a website's **sitemap
//!   URL**" (`handlers/scraping.rs:633-637`) but returns the **page URLs inside** the
//!   sitemap (`scraping.rs:655-676`); the real sitemap-URL discoverer
//!   (`sitemap_discovery.rs:513`) is private. The wire payload is deliberately frozen
//!   (`sitemap_discovery.rs:23-24`, `tests/discover_sitemap_wiremock.rs`), so the
//!   description is the half that moves.
//!
//! Run with: `cargo nextest run -p webfang_mcp --features mcp --test mcp_advertised_defaults_test`
//!
//! Measured red on `08eee306` (pre-fix): 4 of these 8 tests failed — every one of
//! them an advertised-contract defect, not a harness artifact.

#![cfg(feature = "mcp")]

use serde_json::{json, Map, Value};

use webfang_core::config::Config;
use webfang_core::di::{Container, ContainerExt};
use webfang_core::domain::config::ScraperConfig;
use webfang_mcp::mcp_server::handlers::build_tool_router;
use webfang_mcp::mcp_server::handlers::scraping::SCRAPE_BATCH_DEFAULT_CONCURRENCY;
use webfang_mcp::mcp_server::params::{
    ScrapeBatchParams, CONCURRENCY_MAX, CONCURRENCY_MIN, URLS_MAX,
};
use webfang_mcp::mcp_server::schema_bridge::{
    apply_default_overrides, default_overrides_for_tool, merged_input_schema,
};
use webfang_mcp::mcp_server::McpHandler;

/// Tool whose MCP-only parameter is under contract.
const TOOL: &str = "scrape_batch";

/// The property the issue is about.
const CONCURRENCY: &str = "concurrency";

/// The root cause, pinned as a test instead of left as a comment.
///
/// The four `*_input_schema()` helpers the router calls are the only thing
/// standing between the override table and the wire. Re-applying the table to
/// the SERVED properties must therefore be a no-op; when a helper passes `&[]`
/// instead of `default_overrides_for_tool(tool)`, this is the assertion that
/// fails, and it names the tool and the property.
#[test]
fn served_properties_already_carry_the_override_table() {
    let mut served = production_properties();
    let overrides = default_overrides_for_tool(TOOL);
    assert!(
        !overrides.is_empty(),
        "{TOOL} is the tool this suite is about; an empty override table means the \
         production constants were moved and this suite is checking nothing"
    );
    let before = served.clone();
    apply_default_overrides(&mut served, &overrides);
    assert_eq!(
        before, served,
        "{TOOL}: the served schema does not match its own advertised-default \
         override table — a bridge helper is not passing its overrides"
    );
}

/// The `scrape_batch` array parameter under the F7 admission cap (#1611).
const URLS: &str = "urls";

/// Bounds `ScrapeBatchParams::validate` enforces, taken from the production constants
/// so this suite cannot pin a range the validator no longer uses.
const CONCURRENCY_LOWER: u64 = CONCURRENCY_MIN as u64;
const CONCURRENCY_UPPER: u64 = CONCURRENCY_MAX as u64;

/// The concurrency the tool applies when the client omits the field — read from
/// `ScraperConfig::default()`, the expression the handler fell back to before the
/// constant existed, so "advertised equals runtime" compares two independent sources
/// instead of a constant against itself.
fn runtime_default_concurrency() -> u64 {
    ScraperConfig::default().scraper_concurrency as u64
}

/// scrape_batch's input schema exactly as a client is served it.
///
/// This is the ROUTER's emitted `inputSchema` (`build_tool_router` — tool
/// derives plus `apply_overrides`), not a re-merge through
/// `merged_input_schema` (#1612, BC-09). The re-merge was the gap: it
/// applied `default_overrides_for_tool` by hand, so it stayed green while
/// the four `*_input_schema()` helpers the router actually calls passed `&[]`
/// and dropped every override on the floor. `scrape_batch` then served
/// `concurrency` with the derive's `minimum: 0` and no `default` — the exact
/// NS-04 lie this suite exists to prevent, still published.
///
/// The bridge keeps its own invariant test for the merge itself
/// (`every_bridged_tool_applies_its_own_default_overrides`,
/// `schema_bridge::tests`); this side pins the served bytes, so a helper that
/// stops passing its overrides cannot pass here either.
///
/// Returns the `properties` object, not the schema root — the root also carries
/// `$defs`/`required`/`type`, and scanning that instead would silently check nothing
/// (measured: the first run of this suite reported zero prose defaults for exactly
/// this reason).
fn production_properties() -> Map<String, Value> {
    let router = build_tool_router();
    let route = router
        .map
        .get(TOOL)
        .unwrap_or_else(|| panic!("{TOOL} must be registered in the tool router"));
    properties_of(route.attr.input_schema.as_ref())
}

/// The `properties` object of a rendered input schema.
fn properties_of(schema: &Map<String, Value>) -> Map<String, Value> {
    match schema.get("properties") {
        Some(Value::Object(props)) => props.clone(),
        other => panic!("input schema must carry a properties object, got: {other:?}"),
    }
}

/// One advertised property, or a test failure that names the tool.
fn property(name: &str) -> Value {
    let props = production_properties();
    props
        .get(name)
        .unwrap_or_else(|| panic!("{TOOL} must advertise a '{name}' property, got: {props:?}"))
        .clone()
}

/// Characters that end a scalar claimed in prose; see [`prose_default`].
const SCALAR_TERMINATORS: [char; 5] = [' ', ',', ')', '.', ';'];

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
        .find(|c: char| SCALAR_TERMINATORS.contains(&c))
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

/// Control/characterization: what the schemars derive alone advertises for
/// `concurrency`, and therefore where the fix belongs.
///
/// Measured on the current build: **no** `default` key at all, and
/// `"minimum": 0` — the derive's `usize` floor. The bridge's `DefaultOverride`
/// table is the mechanism the repo already uses for this class of drift on
/// `crawl_site` and `delay_ms` (#940 F1/F2), and the bounds need the sibling
/// `SetBounds` sub-slice. Stays green before and after the change.
#[test]
fn derived_schema_alone_advertises_no_concurrency_default() {
    let derived = merged_input_schema::<ScrapeBatchParams>(&[], &[]);
    let props = properties_of(&derived);
    let prop = props.get(CONCURRENCY).unwrap_or_else(|| {
        panic!("concurrency must survive the empty-bridge derive, got: {props:?}")
    });
    assert!(
        prop.get("default").is_none(),
        "an `Option<usize>` field must not gain a machine default from the derive alone; \
         if it does, the override table is the wrong place for the fix: {prop}"
    );
    assert_eq!(
        prop.get("minimum"),
        Some(&json!(0)),
        "the derive floor for usize is 0, which is precisely the lie the bounds \
         sub-slice must overwrite: {prop}"
    );
}

/// NS-04 guard: the advertised default must exist, must equal what the tool applies
/// when the field is omitted, and the constant the bridge advertises must not drift
/// from the config default the handler falls back to.
///
/// Measured red on `08eee306`, where the published property was
/// `{"description":"Concurrency limit (default: 4)","minimum":0}`: no `default` key,
/// prose claiming 4, runtime applying 3.
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
    assert_eq!(
        SCRAPE_BATCH_DEFAULT_CONCURRENCY as u64,
        runtime_default_concurrency(),
        "the constant the bridge advertises has drifted from the config default the \
         handler falls back to"
    );
}

/// NS-04 guard: prose may not claim a default the schema does not carry.
///
/// Every `scrape_batch` property that states `default: <x>` in its description must
/// also expose `default: <x>` as JSON. Measured red on `08eee306` for `concurrency`,
/// which claimed 4 while the schema carried no default at all. After the fix the
/// prose stops naming numbers and the machine-readable field is the only source, so
/// this scan passes by having nothing to contradict. Generalizing it beyond
/// `scrape_batch` is a follow-up, not part of this slice.
#[test]
fn every_prose_default_is_also_a_machine_readable_default() {
    let props = production_properties();
    for (name, prop) in props {
        let Value::Object(prop) = prop else { continue };
        let Some(Value::String(description)) = prop.get("description") else {
            continue;
        };
        let Some(claimed) = prose_default(description) else {
            continue;
        };
        let advertised = prop
            .get("default")
            .and_then(scalar_text)
            .unwrap_or_else(|| {
                panic!(
                    "property '{name}' claims `default: {claimed}` in prose but advertises no \
                     machine-readable default: {prop:?}"
                )
            });
        assert_eq!(
            advertised, claimed,
            "property '{name}' prose default and machine default disagree"
        );
    }
}

/// NS-04 guard: the number lives in exactly one place.
///
/// The `concurrency` description must no longer state a default of its own now that
/// the schema carries the runtime-effective one; a re-added `default: n` here is the
/// drift that produced the original lie.
#[test]
fn concurrency_description_defers_to_the_machine_readable_default() {
    let prop = property(CONCURRENCY);
    let description = prop
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{CONCURRENCY} must be described, got: {prop}"));
    assert!(
        prose_default(description).is_none(),
        "the description must not restate a default the schema already carries: {description:?}"
    );
}

/// NS-04 `SetBounds` sub-slice: the bounds the validator enforces must be
/// discoverable from the schema, not only from a rejected call.
///
/// Measured red on `08eee306`, and worse than absent: the property published
/// `"minimum": 0`, the value `validate()` rejects (#597 pinned the deadlock a
/// `concurrency: 0` caused). The advertised contract told clients to send a value
/// that must fail.
#[test]
fn concurrency_bounds_are_advertised_in_schema() {
    let prop = property(CONCURRENCY);
    assert_eq!(
        prop.get("minimum"),
        Some(&json!(CONCURRENCY_LOWER)),
        "{CONCURRENCY} must advertise its lower bound as enforced by validate(): {prop}"
    );
    assert_eq!(
        prop.get("maximum"),
        Some(&json!(CONCURRENCY_UPPER)),
        "{CONCURRENCY} must advertise its upper bound as enforced by validate(): {prop}"
    );
}

// ============================================================================
// F7 — the `urls` cap is enforced AND advertised from one constant
// ============================================================================

/// F7 (#1611) is the same defect class as NS-04, on the other operand: a
/// validator-only bound is a bound a client cannot see, so an LLM consumer
/// discovers the cap by getting rejected — and, worse, a schema that says
/// "array of any length" while the server refuses is the exact lie #1294
/// called out for `concurrency`.
///
/// Unlike `concurrency`, the advertised side comes from the schemars derive
/// (`#[schemars(extend("maxItems" = URLS_MAX))]`) and not from a
/// `DefaultOverride::SetBounds`: that variant only speaks `minimum` /
/// `maximum`, which are NUMERIC keywords and would be meaningless on an array
/// property. `urls` is MCP-only (it has no OptionsSpec row, exactly like
/// `concurrency`), and `SCRAPE_BATCH_PROPERTIES` leaves it on the derive, so
/// the extension reaches the served schema through the same bridge pass.
#[test]
fn urls_cap_is_advertised_in_schema() {
    let prop = property(URLS);
    assert_eq!(
        prop.get("maxItems"),
        Some(&json!(URLS_MAX)),
        "{URLS} must advertise the cap `validate()` enforces, read from the same \
         constant: {prop}"
    );
}

/// The other half of the same contract, and the reason the assertion above is
/// not a snapshot: the advertised number IS the enforced number. If a future
/// change tightens the validator without touching the schema (or the reverse),
/// this fails even though both sides still look plausible in isolation.
#[test]
fn urls_cap_is_enforced_at_the_advertised_bound() {
    let params = |count: usize| {
        serde_json::json!({
            "urls": (0..count)
                .map(|i| format!("https://example.com/{i}"))
                .collect::<Vec<String>>(),
        })
    };

    serde_json::from_value::<ScrapeBatchParams>(params(URLS_MAX))
        .expect("a batch at the advertised cap must deserialize")
        .validate()
        .expect("a batch at the advertised cap must be accepted");

    let err = serde_json::from_value::<ScrapeBatchParams>(params(URLS_MAX + 1))
        .expect(
            "one URL past the cap still deserializes — the bound is a \
                 validator rule, not a serde one",
        )
        .validate()
        .expect_err("one URL past the advertised cap must be refused");
    assert_eq!(
        err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "got: {err:?}"
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
