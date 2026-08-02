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
                "<html><body><article><h1>{title}</h1></article></body></html>"
            )))
            .mount(server)
            .await;
    }

    base
}

/// A sitemap scrape of two pages with `--trace-file` + vector export must:
/// (a) carry distinct `span_fields.correlation_id` values, one per page, and
/// (b) export the same identities in the vector JSON documents.
#[tokio::test]
async fn scrape_trace_and_vector_export_share_per_page_correlation() {
    let t = BehavioralTest::new().await;
    let base = mount_two_page_site(&t.server).await;

    let trace_path = t.out.path().join("trace.jsonl");

    let output = cmd()
        .arg("--url")
        .arg(&base)
        .arg("--use-sitemap")
        .arg("--export-format")
        .arg("vector")
        .arg("--output")
        .arg(t.out.path())
        .arg("--trace-file")
        .arg(&trace_path)
        .arg("--quiet")
        .output()
        .expect("run webfang binary");

    assert!(
        output.status.success(),
        "expected exit 0, got {:?}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    // --- Parse the JSONL trace ---
    let file = std::fs::File::open(&trace_path)
        .unwrap_or_else(|e| panic!("trace file must exist at {}: {e}", trace_path.display()));
    let lines: Vec<serde_json::Value> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("trace line should be valid JSON: {e}\n{line}"))
        })
        .collect();
    assert!(!lines.is_empty(), "trace must contain events");

    // --- (a) Per-page correlation IDs declared on the scrape spans ---
    let span_correlations: BTreeSet<String> = lines
        .iter()
        .filter_map(|v| {
            v["span_fields"]["correlation_id"]
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    assert!(
        span_correlations.len() >= 2,
        "each scraped page must declare its own correlation_id at span creation (#501); \
         got {} distinct value(s): {span_correlations:?}",
        span_correlations.len()
    );

    // --- (b) The vector export carries the same identities ---
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
