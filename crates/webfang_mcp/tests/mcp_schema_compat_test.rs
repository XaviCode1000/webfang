//! Schema-drift contract tests: what the MCP tools advertise versus what they
//! do (#1612, SD-06 and the no-op fields SD-03/SD-04).
//!
//! The companion to `mcp_advertised_schema_snapshot_test`: that one pins the
//! generated `inputSchema` byte-for-byte, this one pins the *claims* the
//! surface makes around it. A snapshot cannot tell you a description is a lie;
//! it only records that the lie is stable.
//!
//! The invariant here is narrow on purpose. It is not "descriptions must be
//! nice" — it is: **a tool description may not advertise a control the tool's
//! own input schema does not have.** That is SD-06 exactly, and it is
//! mechanically checkable, so it does not depend on a reviewer noticing.

use serde_json::Value;
use webfang_mcp::mcp_server::handlers::build_tool_router;

/// Control parameters whose mention in a tool description is a *behavioural*
/// claim — "this tool runs N at a time", "this tool can be paced", "this tool
/// fetches N pages" — rather than a topic word.
///
/// These are the literal wire names, so the check is exact: a description may
/// name one only if the tool's own input schema accepts it. A prose synonym
/// ("rate limiting" for `delay_ms`) is deliberately NOT in the list; a review
/// catches that, a substring match cannot.
const CONTROL_WORDS: &[&str] = &["concurrency", "delay_ms", "single_page", "max_pages"];

/// One advertised tool: its name, its client-facing description, and its
/// property names.
struct AdvertisedTool {
    name: String,
    description: String,
    properties: Vec<String>,
}

fn advertised_tools() -> Vec<AdvertisedTool> {
    build_tool_router()
        .map
        .into_iter()
        .map(|(name, route)| {
            let attr = route.attr;
            let description = attr.description.clone().unwrap_or_default().to_string();
            let properties = attr
                .input_schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|props| props.keys().cloned().collect())
                .unwrap_or_default();
            AdvertisedTool {
                name: name.to_string(),
                description,
                properties,
            }
        })
        .collect()
}

fn tool(name: &str) -> AdvertisedTool {
    advertised_tools()
        .into_iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("{name} must be registered"))
}

/// SD-06: `scrape_with_options` advertised "asset downloading, concurrency, and
/// delay settings". Its params are `url`, `max_pages`, `download_images`,
/// `download_documents`, `selector` and `ignore_robots` — there is no
/// concurrency and no delay knob to turn. The tool handler builds a
/// `ScraperConfig` and calls `scrape_with_config`, which fetches one page and
/// does no pacing; `concurrency` and `delay_ms` live on `scrape_batch`, which
/// is a different tool.
///
/// The generic check below is what makes this test redundant-by-design: if a
/// future description re-advertises an absent control, it fails without anyone
/// remembering this one.
#[test]
fn scrape_with_options_does_not_advertise_controls_it_lacks() {
    let advertised = tool("scrape_with_options");
    let lowered = advertised.description.to_lowercase();

    for control in CONTROL_WORDS {
        if lowered.contains(control) {
            assert!(
                advertised.properties.iter().any(|p| p == control),
                "scrape_with_options advertises `{control}` in its description but its \
                 input schema offers no such control (properties: {:?}). Either the \
                 description or the params are wrong.",
                advertised.properties
            );
        }
    }

    // The replacement text must still say what the tool DOES, and point at
    // the tools that do offer the controls it no longer claims.
    assert!(
        lowered.contains("crawl_site"),
        "the corrected description must route multi-page callers to crawl_site: {:?}",
        advertised.description
    );
    assert!(
        lowered.contains("scrape_batch"),
        "the corrected description must route paced/concurrent callers to scrape_batch: {:?}",
        advertised.description
    );
}

/// The same defect, second instance, found by the generic check rather than by
/// the audit: `crawl_site` advertised "configurable depth limit, concurrency
/// control, and rate limiting", and `CrawlSiteParams` has neither a
/// concurrency nor a pacing field — the engine derives both from the shared
/// budget model. The bridge comment for this tool even listed `concurrency`
/// as an MCP-only property "like `concurrency`", which is how a description
/// nobody re-reads outlives the params it described.
#[test]
fn crawl_site_does_not_advertise_controls_it_lacks() {
    let advertised = tool("crawl_site");
    let lowered = advertised.description.to_lowercase();
    for control in CONTROL_WORDS {
        if lowered.contains(control) {
            assert!(
                advertised.properties.iter().any(|p| p == control),
                "crawl_site advertises `{control}` but does not accept it (properties: {:?})",
                advertised.properties
            );
        }
    }
    assert!(
        lowered.contains("budget model"),
        "the corrected description must say where parallelism actually comes from: {:?}",
        advertised.description
    );
}

/// The same invariant for every registered tool, not just the one the audit
/// named. A description that advertises a control its own schema lacks sends
/// the caller to a parameter that `deny_unknown_fields` will reject.
#[test]
fn no_tool_advertises_a_control_its_schema_lacks() {
    for tool in advertised_tools() {
        let lowered = tool.description.to_lowercase();
        for control in CONTROL_WORDS {
            if lowered.contains(control) {
                assert!(
                    tool.properties.iter().any(|p| p == control),
                    "{} advertises `{control}` in its description but does not accept that \
                     parameter (properties: {:?})",
                    tool.name,
                    tool.properties
                );
            }
        }
    }
}
