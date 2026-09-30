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

use serde_json::{json, Value};
use webfang_mcp::mcp_server::handlers::build_tool_router;
use webfang_mcp::mcp_server::params::{
    ExportFileParams, ProcessExportPipelineParams, ScrapeBatchParams, ScrapeWithOptionsParams,
};

/// A field the tool accepts, bounds-checks, advertises -- and never reads.
///
/// The two instances (#1612 SD-03, SD-04; the pair VG-04 calls out as
/// "validation implies control that does not exist"):
///
/// - `scrape_with_options.max_pages` — the handler copies it into
///   `ScraperConfig` and `scraper_service::scrape_with_config` never reads
///   the field; the tool fetches one URL and does no discovery.
/// - `scrape_batch.single_page` — never read at all. Batch scraping is
///   single-page by construction (#1215), so there is no crawl-expansion
///   mode for the flag to disable.
const NO_OP_FIELDS: &[(&str, &str)] = &[
    ("scrape_with_options", "max_pages"),
    ("scrape_batch", "single_page"),
];

fn advertised_properties(tool: &str) -> Vec<(String, Value)> {
    let router = build_tool_router();
    let route = router
        .map
        .get(tool)
        .unwrap_or_else(|| panic!("tool {tool} must be registered"));
    route
        .attr
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|props| props.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn property(tool: &str, name: &str) -> Value {
    advertised_properties(tool)
        .into_iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("{tool} must advertise `{name}`"))
}

/// SD-03/SD-04, decided rather than deferred wholesale.
///
/// Neither field can be REMOVED here — dropping an advertised field, or
/// turning `single_page: false` into a rejection, breaks MCP consumers, and
/// that is #1614's call. What this issue can do without a compat decision is
/// stop the advertisement from implying a control the handler does not
/// implement: each no-op field now carries a per-tool description that says
/// it has no effect and names the tool that does the thing.
///
/// This test is the decision's record. If someone later implements the field,
/// the description it contradicts fails here and the test is deleted with the
/// change rather than left as a lie in the other direction.
#[test]
fn advertised_no_op_fields_declare_themselves_inert() {
    for (tool_name, field) in NO_OP_FIELDS {
        let prop = property(tool_name, field);
        let description = prop["description"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool_name}.{field} must be described: {prop}"));
        assert!(
            description.contains("no effect"),
            "{tool_name}.{field} is a no-op (the handler never reads it) but its \
             advertised description does not say so, so the schema still implies a \
             control that does not exist: {description:?}"
        );
    }
}

/// The other half of the same record: the fields stay ACCEPTED.
///
/// A description that says "no effect" is only honest if passing the field is
/// still a valid call. If a future change starts rejecting `max_pages`, this
/// fails and the rejection has to arrive as a deliberate, reviewed decision
/// rather than as an accident.
#[test]
fn advertised_no_op_fields_are_still_accepted() {
    let scrape_with_options = serde_json::from_value::<ScrapeWithOptionsParams>(json!({
        "url": "https://example.com/",
        "max_pages": 5,
    }))
    .expect("scrape_with_options must still accept max_pages");
    scrape_with_options
        .validate()
        .expect("an in-bounds max_pages must still validate");
    assert_eq!(scrape_with_options.max_pages, Some(5));

    let scrape_batch = serde_json::from_value::<ScrapeBatchParams>(json!({
        "urls": ["https://example.com/"],
        "single_page": false,
    }))
    .expect("scrape_batch must still accept single_page");
    scrape_batch
        .validate()
        .expect("a batch with single_page must still validate");
    assert_eq!(scrape_batch.single_page, Some(false));
}

/// The bound on the inert `max_pages` is still enforced while it is inert.///
/// VG-04's complaint is that validation implies control; leaving the bound in
/// place is deliberate, because removing it is a behaviour change and
/// advertising a field is not the same as validating it. This pins that the
/// two halves did not drift apart: an in-bounds value is accepted, an
/// out-of-bounds one is still refused with the published `invalid_params`
/// channel.
#[test]
fn inert_max_pages_keeps_its_spec_bound() {
    let out_of_bounds = ScrapeWithOptionsParams {
        url: "https://example.com/".parse().expect("valid url"),
        max_pages: Some(100_001),
        download_images: None,
        download_documents: None,
        selector: None,
        ignore_robots: None,
    };
    let err = out_of_bounds
        .validate()
        .expect_err("max_pages above the spec cap must still be refused");
    assert_eq!(
        err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "the published invalid_params channel must not change: {err:?}"
    );
}

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

// ============================================================================
// SD-01 / SD-02 / BC-01 — the export wire-name matrix
// ============================================================================

/// One export tool's name story, declared once.
///
/// `advertised` is what the served schema lists; `accepted` is what serde
/// actually deserializes. The two sets are the whole of SD-01/SD-02, and
/// declaring them side by side is what turns the remaining gap into a
/// decision rather than a drift nobody can see.
struct ExportTool {
    /// Registered tool name.
    tool: &'static str,
    /// Wire names the advertised input schema lists.
    advertised: &'static [&'static str],
    /// Wire names the params struct actually accepts.
    accepted: &'static [&'static str],
    /// Accepted but not advertised. Each entry is a reviewed, recorded gap --
    /// see [`unadvertised_export_alias_is_declared_and_still_works`].
    accepted_not_advertised: &'static [&'static str],
    /// A minimal call for the tool with NO format key, so each test controls
    /// exactly which name gets inserted.
    base: fn() -> serde_json::Value,
}

fn export_file_call() -> serde_json::Value {
    json!({
        "output_dir": "/tmp/export",
        "filename": "note",
        "content": "body",
    })
}

fn pipeline_call() -> serde_json::Value {
    json!({})
}

const EXPORT_TOOLS: &[ExportTool] = &[
    ExportTool {
        tool: "export_file",
        advertised: &["content_format", "format"],
        accepted: &["content_format", "format"],
        // Nothing: this tool never accepted the spec id `export_format`.
        accepted_not_advertised: &[],
        base: export_file_call,
    },
    ExportTool {
        tool: "process_export_pipeline",
        advertised: &["pipeline_format", "format"],
        accepted: &["pipeline_format", "format", "export_format"],
        // SD-02. Deliberately left unadvertised -- see the test doc.
        accepted_not_advertised: &["export_format"],
        base: pipeline_call,
    },
];

/// Every wire name any export tool in the matrix knows about. Used to pick the
/// format-related properties out of a served schema whose other properties
/// (`url`, `output_dir`, ...) are not part of this story.
fn format_name_universe() -> Vec<&'static str> {
    EXPORT_TOOLS
        .iter()
        .flat_map(|e| e.accepted.iter().copied())
        .collect()
}

/// The format-related properties the served schema lists, sorted.
fn served_format_names(tool: &str) -> Vec<String> {
    let universe = format_name_universe();
    let mut names: Vec<String> = advertised_properties(tool)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|p| universe.contains(&p.as_str()))
        .collect();
    // Map order is not a contract; sort so the comparison is stable.
    names.sort();
    names
}

/// BC-01: the advertised format properties of each export tool are exactly the
/// declared set, read from the served router rather than from a table the test
/// trusts.
#[test]
fn export_wire_name_matrix_is_pinned() {
    for entry in EXPORT_TOOLS {
        let mut expected: Vec<String> = entry.advertised.iter().map(|s| (*s).to_owned()).collect();
        expected.sort();
        assert_eq!(
            served_format_names(entry.tool),
            expected,
            "{}: the served format properties are not the declared set",
            entry.tool
        );
    }
}

/// Every declared accepted name deserializes on its own, carrying the value the
/// caller sent.
///
/// This is the positive half. A name listed in the matrix and silently rejected
/// is exactly the drift the matrix exists to catch, and `deny_unknown_fields`
/// makes an unlisted name fail loudly -- so a name that works here and is
/// absent from `accepted` is a hole in the table, not a tolerance.
#[test]
fn every_declared_export_alias_deserializes_alone() {
    for entry in EXPORT_TOOLS {
        for name in entry.accepted {
            let mut call = (entry.base)();
            call.as_object_mut()
                .expect("call is an object")
                .insert((*name).to_owned(), json!("jsonl"));
            let ok = match entry.tool {
                "export_file" => serde_json::from_value::<ExportFileParams>(call.clone()).is_ok(),
                "process_export_pipeline" => {
                    serde_json::from_value::<ProcessExportPipelineParams>(call.clone()).is_ok()
                },
                other => panic!("no deserializer declared for {other}"),
            };
            assert!(ok, "{} must accept `{name}` on its own: {call}", entry.tool);
        }
    }
}

/// The half the audit could not see, and the reason the descriptions above had
/// to change: two accepted names in one call are a **duplicate-field error**,
/// not a merge and not a last-wins. Measured here, not assumed.
///
/// `export_file` is the sharp case. Its served schema marks `content_format`
/// `required` and ALSO advertises `format` as an ordinary optional property,
/// so a client that follows the schema exactly and sets the optional one
/// alongside the required one gets a hard failure that no schema rule forbids.
///
/// **Deferred to #1614.** Every repair is compat-bearing: dropping either
/// advertised name, or relaxing the struct so duplicates resolve instead of
/// failing, both change what an existing client can send. The
/// `deny_unknown_fields` guard this relies on is deliberate -- the module docs
/// on `params.rs` name it as the defence against typosquat keys -- so loosening
/// it is not this issue's call.
#[test]
fn two_export_format_names_in_one_call_is_a_duplicate_field_error() {
    for entry in EXPORT_TOOLS {
        let first = entry.accepted[0];
        let second = entry.accepted[1];
        let mut call = (entry.base)();
        {
            let object = call.as_object_mut().expect("call is an object");
            object.insert(first.to_owned(), json!("jsonl"));
            object.insert(second.to_owned(), json!("jsonl"));
        }
        let err = match entry.tool {
            "export_file" => serde_json::from_value::<ExportFileParams>(call.clone())
                .expect_err("two names for one field must not silently succeed")
                .to_string(),
            "process_export_pipeline" => {
                serde_json::from_value::<ProcessExportPipelineParams>(call.clone())
                    .expect_err("two names for one field must not silently succeed")
                    .to_string()
            },
            other => panic!("no deserializer declared for {other}"),
        };
        assert!(
            err.contains("duplicate field"),
            "{}: sending `{first}` and `{second}` must fail as a duplicate field, got: {err}",
            entry.tool
        );
    }
}

/// The recorded gap: `export_format` deserializes on
/// `process_export_pipeline` but is not advertised.
///
/// It is listed so the set difference is asserted in one place. If #1614
/// collapses the three spellings into one, this row is what gets deleted;
/// until then it is a known, named, tested gap rather than drift. The second
/// half asserts the reverse direction too: nothing advertised that is not
/// accepted, which would be a call the schema invites and the server refuses.
#[test]
fn unadvertised_export_alias_is_declared_and_still_works() {
    for entry in EXPORT_TOOLS {
        let served = served_format_names(entry.tool);
        for name in entry.accepted_not_advertised {
            assert!(
                !served.iter().any(|p| p == name),
                "{} advertises `{name}` now; move it into `advertised` and drop it \
                 from accepted_not_advertised",
                entry.tool
            );
        }
        for name in entry.advertised {
            assert!(
                entry.accepted.contains(name),
                "{} advertises `{name}` but does not accept it",
                entry.tool
            );
        }
    }
}

/// The enum behind every spelling is the OptionsSpec's, so the three names
/// cannot drift apart on WHICH values they take.
///
/// `export_formats()` in `params.rs` already derives the closed set from
/// `export::EXPORT_FORMAT`; this asserts the schemas publish that same set, so
/// a spec change shows up in the served contract instead of only in the
/// validator.
#[test]
fn every_advertised_export_format_publishes_the_spec_enum() {
    for entry in EXPORT_TOOLS {
        for name in entry.advertised {
            let prop = property(entry.tool, name);
            // The derive-rendered field (content_format / pipeline_format) is a
            // bare `string`; only the spec-backed row carries the closed enum.
            // Where it does, it must be the spec's.
            if let Some(advertised) = prop.get("enum") {
                assert_eq!(
                    advertised,
                    &json!(["jsonl", "vector", "auto"]),
                    "{}.{name} must publish the OptionsSpec enum",
                    entry.tool
                );
            }
        }
    }
}
