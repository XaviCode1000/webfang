//! End-to-end trace correlation (issue #501).
//!
//! Acceptance criterion from the issue: a multi-page scrape with
//! `--trace-file` must emit per-page `correlation_id` values in
//! `span_fields` (one distinct identity per page), and the identities
//! exported to the RAG vector export must match them — proving the trace
//! JSONL and the exported content are correlatable.

use crate::cmd;
use crate::BehavioralTest;
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

/// Two scrapeable pages listed in the sitemap, plus an allow-all robots.txt.
async fn mount_two_page_site(server: &wiremock::MockServer) -> String {
    let base = server.uri();

    crate::common::mock_robots(server, "User-agent: *\n").await;

    let sitemap_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>{base}/page-a</loc></url>
    <url><loc>{base}/page-b</loc></url>
</urlset>"#
    );
    crate::common::mock_sitemap(server, &format!("{base}/sitemap.xml"), &sitemap_xml).await;

    // Sitemap discovery probes fallback locations with HEAD before parsing
    // them with GET (sitemap_discovery.rs), so the probe needs its own mock.
    Mock::given(method("HEAD"))
        .and(path("/sitemap.xml"))
        .respond_with(ResponseTemplate::new(200))
        .mount(server)
        .await;

    for (page_path, title) in [("/page-a", "Page A"), ("/page-b", "Page B")] {
        Mock::given(method("GET"))
            .and(path(page_path))
            .respond_with(ResponseTemplate::new(200).set_body_string(format!(
                "<html><body><article><h1>{title}</h1>\
                 <p>Substantive body for {title}, long enough to clear the fifty character \
                 minimum content guard so the scrape succeeds deterministically.</p>\
                 </article></body></html>"
            )))
            .mount(server)
            .await;
    }

    base
}

/// A sitemap scrape of two pages with `--trace-file` + vector export must:
/// (a) carry distinct `span_fields.correlation_id` values, one per page,
/// (b) share exactly ONE run-root `span_fields.trace_id` across all page
///     spans, with every traceparent embedding that same trace UUID, and
/// (c) export the same identities in the vector JSON documents.
/// Run a sitemap scrape with `--trace-file` + vector export and return the
/// child process output. Vector export requires `--clean-ai` (preflight #796),
/// so the command needs a cached ONNX model.
fn run_scrape_with_trace(
    base: &str,
    output_dir: &std::path::Path,
    trace_path: &std::path::Path,
) -> std::process::Output {
    cmd()
        .arg("--url")
        .arg(base)
        .arg("--use-sitemap")
        .arg("--export-format")
        .arg("vector")
        .arg("--clean-ai")
        .arg("--output")
        .arg(output_dir)
        .arg("--trace-file")
        .arg(trace_path)
        .arg("--quiet")
        .output()
        .expect("run webfang binary")
}

/// Read a JSONL trace file into a `Vec` of parsed JSON events.
fn parse_trace(trace_path: &std::path::Path) -> Vec<serde_json::Value> {
    let file = std::fs::File::open(trace_path)
        .unwrap_or_else(|e| panic!("trace file must exist at {}: {e}", trace_path.display()));
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("trace line should be valid JSON: {e}\n{line}"))
        })
        .collect()
}

/// Collect the distinct per-page `span_fields.correlation_id` values.
fn span_correlation_ids(lines: &[serde_json::Value]) -> BTreeSet<String> {
    lines
        .iter()
        .filter_map(|v| {
            v["span_fields"]["correlation_id"]
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}

/// (a) Each scraped page must declare its own correlation_id at span creation.
fn assert_per_page_correlation_ids(lines: &[serde_json::Value]) {
    let span_correlations = span_correlation_ids(lines);
    assert!(
        span_correlations.len() >= 2,
        "each scraped page must declare its own correlation_id at span creation (#501); \
         got {} distinct value(s): {span_correlations:?}",
        span_correlations.len()
    );
}

/// (b) All page spans must share ONE run-root trace_id; returns it.
fn assert_shared_run_root_trace(lines: &[serde_json::Value]) -> String {
    let span_traces: BTreeSet<String> = lines
        .iter()
        .filter_map(|v| v["span_fields"]["trace_id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        span_traces.len(),
        1,
        "every page span of a run must share the run-root trace_id (causality); \
         got {} distinct value(s): {span_traces:?}",
        span_traces.len()
    );
    span_traces
        .first()
        .expect("trace set asserted non-empty above")
        .clone()
}

/// Every per-page traceparent `00-{32hex trace}-{16hex span}-01` must embed
/// the same run-root trace UUID (without dashes) as its trace part.
fn assert_traceparent_embeds_run_trace(lines: &[serde_json::Value], run_trace: &str) {
    let span_correlations = span_correlation_ids(lines);
    let run_trace_hex = run_trace.replace('-', "");
    for corr in &span_correlations {
        let parts: Vec<&str> = corr.split('-').collect();
        assert_eq!(
            parts.len(),
            4,
            "correlation_id must be a W3C traceparent `00-<trace>-<span>-01`, got: {corr}"
        );
        assert_eq!(parts[0], "00", "traceparent version must be 00: {corr}");
        assert_eq!(parts[3], "01", "traceparent flags must be 01: {corr}");
        assert_eq!(
            parts[1].len(),
            32,
            "traceparent trace part must be 32 hex chars: {corr}"
        );
        assert_eq!(
            parts[2].len(),
            16,
            "traceparent span part must be 16 hex chars: {corr}"
        );
        assert_eq!(
            parts[1], run_trace_hex,
            "traceparent trace part must equal the shared run-root trace_id \
             ({run_trace}); page identity drifted: {corr}"
        );
    }
}

/// (c) The vector export must carry the same correlation identities.
fn assert_vector_export_identities(t: &BehavioralTest, lines: &[serde_json::Value]) {
    let span_correlations = span_correlation_ids(lines);

    let vector_files = t.find_files("json");
    assert!(
        !vector_files.is_empty(),
        "vector export must produce a .json file"
    );
    let export_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&vector_files[0]).expect("read vector export"),
    )
    .expect("vector export must be valid JSON");
    let documents = export_json["documents"]
        .as_array()
        .expect("vector export must contain documents");
    assert_eq!(
        documents.len(),
        2,
        "both sitemap pages must be exported, got {}",
        documents.len()
    );

    let mut exported: BTreeSet<String> = BTreeSet::new();
    for doc in documents {
        let corr = doc["correlation_id"]
            .as_str()
            .expect("every exported document must carry a correlation_id");
        assert!(
            span_correlations.contains(corr),
            "exported correlation_id {corr} must appear in the trace span_fields \
             ({span_correlations:?})"
        );
        exported.insert(corr.to_owned());
    }
    assert_eq!(
        exported.len(),
        2,
        "each page must keep its own identity in the export; got {exported:?}"
    );
}

/// Vector export requires `--clean-ai`, hence a cached ONNX model (preflight #796).
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn scrape_trace_and_vector_export_share_per_page_correlation() {
    let t = BehavioralTest::new().await;
    let base = mount_two_page_site(&t.server).await;

    let trace_path = t.out.path().join("trace.jsonl");

    let output = run_scrape_with_trace(&base, t.out.path(), &trace_path);
    assert!(
        output.status.success(),
        "expected exit 0, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    // --- Parse the JSONL trace ---
    let lines = parse_trace(&trace_path);
    assert!(!lines.is_empty(), "trace must contain events");

    // --- (a) Per-page correlation IDs declared on the scrape spans ---
    assert_per_page_correlation_ids(&lines);

    // --- (b) All page spans share ONE run-root trace_id (#501 follow-up) ---
    let run_trace = assert_shared_run_root_trace(&lines);
    assert_traceparent_embeds_run_trace(&lines, &run_trace);

    // --- (c) The vector export carries the same identities ---
    assert_vector_export_identities(&t, &lines);
}

// ——— #1439: DOM-route run-root regression pin ———
//
// The single-fetch/sitemap route was sealed by #1412; the DOM route that
// enters the Engine through the PUBLIC wrapper (`crawl_site_with_options`)
// kept re-breaking the invariant (#687 fixed Aug, re-broken by the #1232
// rework): the wrapper minted its own run-root milliseconds after the CLI
// announced one, so reconstructing the run by the announced trace_id
// silently missed discovery + crawl. This test pins the DOM route with NO
// model dependency — it must run everywhere, hence it ships un-skipped.

/// Collect the `trace_id` values the run announced via "run identity"
/// events (top-level `message`, durable UUID in `.fields.trace_id`).
/// Both the CLI entry event and every `CrawlSession::begin` event land
/// here; #1439 requires they all agree.
fn run_identity_trace_ids(lines: &[serde_json::Value]) -> BTreeSet<String> {
    lines
        .iter()
        .filter(|v| v["message"].as_str() == Some("run identity"))
        .filter_map(|v| v["fields"]["trace_id"].as_str().map(str::to_owned))
        .collect()
}

/// Mount a pure link graph — NO sitemap: `/` links `/page-a`, which links
/// `/page-b`. Padded bodies clear the 50-char minimum-content guard so the
/// scrape phase succeeds deterministically.
async fn mount_dom_link_chain(t: &BehavioralTest) -> String {
    let pad = "carries plenty of substantive server-rendered text so it comfortably \
               clears the fifty character minimum content guard.";
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<html><body><a href="/page-a">A</a><p>This hub page {pad}</p></body></html>"#
        )))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page-a"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<html><body><h1>Page A</h1><a href="/page-b">B</a><p>This page A {pad}</p></body></html>"#
        )))
        .mount(&t.server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            r#"<html><body><article><h1>Page B</h1><p>This page B {pad}</p></article></body></html>"#
        )))
        .mount(&t.server)
        .await;

    t.server.uri()
}

/// A DOM crawl (`--max-depth 2`, link graph, no sitemap) with `--trace-file`
/// must reconstruct from ONE run root:
/// (i)   exactly one distinct `span_fields.trace_id` across the whole JSONL,
///       and every "run identity" event announces that same UUID;
/// (ii)  at least two page spans with distinct `correlation_id` values
///       (children of the root — one identity per page, #501);
/// (iii) every per-page traceparent embeds the run-root trace UUID.
#[tokio::test]
async fn dom_crawl_trace_shares_one_run_root() {
    let t = BehavioralTest::new().await;
    let base = mount_dom_link_chain(&t).await;

    let trace_path = t.out.path().join("trace.jsonl");
    let output = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--max-depth")
        .arg("2")
        .arg("--ignore-robots")
        .arg("--output")
        .arg(t.out.path())
        .arg("--trace-file")
        .arg(&trace_path)
        .arg("--quiet")
        .output()
        .expect("run webfang binary");
    assert!(
        output.status.success(),
        "DOM depth-2 crawl must succeed, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let lines = parse_trace(&trace_path);
    assert!(!lines.is_empty(), "trace must contain events");

    // (i) ONE distinct span trace_id, and the announced identities agree.
    let run_trace = assert_shared_run_root_trace(&lines);
    let announced = run_identity_trace_ids(&lines);
    assert_eq!(
        announced,
        BTreeSet::from([run_trace.clone()]),
        "every 'run identity' event must announce the single run root — a second \
         value means the Engine minted its own identity beside the CLI's (#1439)"
    );

    // (ii) at least two page identities, each its own span (#501).
    assert_per_page_correlation_ids(&lines);

    // (iii) all page traceparents embed the run-root trace UUID.
    assert_traceparent_embeds_run_trace(&lines, &run_trace);
}
