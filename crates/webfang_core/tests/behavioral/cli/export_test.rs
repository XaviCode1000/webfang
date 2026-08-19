//! RAG export behavioral tests — vector export header invariants (issue #502).

use crate::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const PAGE_HTML: &str = r#"
<html><head><title>Vector Export Test</title></head>
<body><main><article>
<h1>Export Me</h1>
<p>Enough content for the extractor to produce a real document chunk.</p>
<p>A second paragraph keeps readability extraction stable and deterministic.</p>
</article></main></body></html>
"#;

/// Issue #502 repro: the vector export header `total_documents` must equal
/// the number of entries actually present in the `documents` array.
///
/// Semantic JSON assertions instead of snapshots on purpose: the header embeds
/// a `created_at` timestamp that is non-deterministic by design.
///
/// Requires a cached ONNX model: `--export-format vector` needs `--clean-ai`
/// (preflight #796).
#[tokio::test]
#[ignore = "requires cached ONNX model"]
async fn vector_export_total_documents_matches_documents() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--export-format")
        .arg("vector")
        .arg("--clean-ai")
        .arg("--quiet")
        .assert()
        .success();

    let export_path = t.out.path().join("export.json");
    let content = std::fs::read_to_string(&export_path).expect("vector export file should exist");
    let json: serde_json::Value =
        serde_json::from_str(&content).expect("export must be valid JSON");

    let total = json["total_documents"]
        .as_u64()
        .expect("total_documents must be a number");
    let documents = json["documents"]
        .as_array()
        .expect("documents must be an array");

    assert!(
        !documents.is_empty(),
        "at least one document should be exported"
    );
    assert_eq!(
        total,
        documents.len() as u64,
        "header total_documents must match the documents array length"
    );
}
