//! Sitemap observability contract (issue #1318).
//!
//! The sitemap-index failure path must report through `log_scrape_error`:
//! every child failure event carries `url`, `stage` and the run's durable
//! correlation in the `--trace-file` JSONL — reconstructable by `trace_id`
//! without scraping the human message for the URL. This test asserts field
//! shape on the trace JSONL, not stderr text.

use crate::cmd;
use std::io::{BufRead, BufReader};
use std::path::Path;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Article HTML comfortably over the 50-char minimum content guard.
const ARTICLE_HTML: &str = "<html><body><article>\
     <h1>Sitemap Child Page</h1>\
     <p>Substantive content from a sitemap-listed page, long enough to clear \
     the fifty character minimum content guard comfortably for a green run.</p>\
     </article></body></html>";

fn sitemap_index_with(locs: &[String]) -> String {
    let urls: String = locs
        .iter()
        .map(|loc| format!("<sitemap><loc>{loc}</loc></sitemap>"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</sitemapindex>"#
    )
}

fn urlset_with(locs: &[String]) -> String {
    let urls: String = locs
        .iter()
        .map(|loc| format!("<url><loc>{loc}</loc></url>"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">{urls}</urlset>"#
    )
}

/// Read a JSONL trace file into a `Vec` of parsed JSON events.
fn parse_trace(trace_path: &Path) -> Vec<serde_json::Value> {
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

/// A durable correlation `trace_id` as logged by `log_scrape_error`: the
/// UUID string form of the correlation's trace (8-4-4-4-12 hex groups).
fn assert_trace_uuid_shape(value: Option<&serde_json::Value>, what: &str) -> String {
    let raw = value
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("{what} must carry a string trace_id field"));
    let groups: Vec<&str> = raw.split('-').collect();
    assert_eq!(
        groups.len(),
        5,
        "{what} must carry a UUID-shaped trace_id, got {raw:?}"
    );
    assert!(
        groups
            .iter()
            .map(|g| g.len())
            .eq([8, 4, 4, 4, 12].iter().copied()),
        "{what} trace_id must follow the 8-4-4-4-12 UUID shape, got {raw:?}"
    );
    raw.to_owned()
}

/// Mount a sitemap index whose first child fails with 500 and whose second
/// lists one scrapeable page. Returns the failing child's absolute URL.
async fn mount_flaky_index_site(server: &MockServer) -> String {
    let base = server.uri();
    let child_a = format!("{base}/child-a.xml");
    let child_b = format!("{base}/child-b.xml");

    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\n"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(sitemap_index_with(&[child_a.clone(), child_b])),
        )
        .mount(server)
        .await;
    // The first index child always fails with a 500; the second yields a page.
    Mock::given(method("GET"))
        .and(path("/child-a.xml"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/child-b.xml"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(urlset_with(&[format!("{base}/page-b")])),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/page-b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(server)
        .await;

    child_a
}

#[tokio::test]
async fn sitemap_child_failure_carries_stage_and_correlation() {
    let server = MockServer::start().await;
    let output = TempDir::new().unwrap();
    let base = server.uri();
    let trace_path = output.path().join("trace.jsonl");
    let child_a = mount_flaky_index_site(&server).await;

    let result = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--max-retries")
        .arg("0")
        .arg("--output")
        .arg(output.path())
        .arg("--trace-file")
        .arg(&trace_path)
        .arg("--quiet")
        .output()
        .expect("run webfang");

    assert!(
        result.status.success(),
        "one failing index child must not sink the run (the other child lists a page); \
         stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );

    let lines = parse_trace(&trace_path);

    // Every ERROR event for the failing child URL must carry a `stage` and a
    // durable correlation trace_id (#1318: the pre-fix `warn!`s had neither).
    let child_events: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|v| {
            v["level"].as_str() == Some("ERROR") && v["fields"]["url"].as_str() == Some(&child_a)
        })
        .collect();
    assert!(
        !child_events.is_empty(),
        "the failing index child must produce ERROR events in the trace JSONL"
    );

    let mut stages: Vec<&str> = Vec::new();
    for event in &child_events {
        let stage = event["fields"]["stage"]
            .as_str()
            .unwrap_or_else(|| panic!("every error-path event must carry `stage`: {event}"));
        stages.push(stage);
        assert_trace_uuid_shape(event["fields"].get("trace_id"), "child error event");
    }

    // The two mandatory sites of the index-child failure path: where the child
    // fetch was rejected, and where the index aggregated the child failure.
    assert!(
        stages.contains(&"sitemap.fetch"),
        "the child's non-2xx rejection must be logged at stage `sitemap.fetch`, got {stages:?}"
    );
    assert!(
        stages.contains(&"sitemap.index_child"),
        "the index must aggregate the child failure at stage `sitemap.index_child`, got {stages:?}"
    );

    // The fetch-rejection event must be joinable with the child's parse span:
    // the durable `trace_id` it logs equals its own `span_fields.trace_id`
    // (same statement shared by both the instrumented span and the error).
    let fetch_event = child_events
        .iter()
        .find(|v| v["fields"]["stage"].as_str() == Some("sitemap.fetch"))
        .expect("sitemap.fetch event asserted above");
    let event_trace = fetch_event["fields"]["trace_id"]
        .as_str()
        .expect("stage event trace_id asserted above");
    let span_trace = fetch_event["span_fields"]["trace_id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "the failing child fetch must sit under a span carrying its \
             correlation trace_id; span_fields: {:?}",
                fetch_event["span_fields"]
            )
        });
    assert_eq!(
        event_trace, span_trace,
        "the error event's trace_id must match the correlation trace_id of the child parse span"
    );
}
