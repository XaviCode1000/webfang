//! DOM pre-pruning behavior tests (#791).
//!
//! Verifies that semantic DOM pruning removes invisible/empty elements
//! before Readability extraction, and that the --dom-preprune flag works.

use crate::BehavioralTest;
use predicates::prelude::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const HIDDEN_ELEMENT_HTML: &str = r#"
<html><head><title>Hidden Elements Test</title></head>
<body><main><article>
<h1>Visible Article Title</h1>
<p>This visible paragraph must survive the pre-pruning pass unchanged.</p>
<div style="display: none">This hidden div content must be removed by pre-pruning.</div>
<p>Another visible paragraph to keep the extractor happy.</p>
</article></main></body></html>
"#;

const VISIBILITY_HIDDEN_HTML: &str = r#"
<html><head><title>Visibility Hidden Test</title></head>
<body><main><article>
<h1>Visibility Test Article</h1>
<span style="visibility: hidden">Invisible span content should be pruned away.</span>
<p>This visible paragraph must be preserved in the extraction output.</p>
<p>Final paragraph for extraction closure.</p>
</article></main></body></html>
"#;

// ---------------------------------------------------------------------------
// display:none removal (default: pruning enabled)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dom_pruning_removes_display_none_content() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HIDDEN_ELEMENT_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    crate::assert_snapshot_redacted(
        "dom_pruning_removes_display_none_content",
        t.out.path(),
        &content,
    );
    assert!(
        !content.contains("hidden div content"),
        "display:none content must be removed by pre-pruning"
    );
    assert!(
        content.contains("visible paragraph"),
        "visible content must be preserved"
    );
}

// ---------------------------------------------------------------------------
// visibility:hidden removal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dom_pruning_removes_visibility_hidden_content() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(VISIBILITY_HIDDEN_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    crate::assert_snapshot_redacted(
        "dom_pruning_removes_visibility_hidden_content",
        t.out.path(),
        &content,
    );
    assert!(
        !content.contains("Invisible span content"),
        "visibility:hidden content must be removed by pre-pruning"
    );
}

// ---------------------------------------------------------------------------
// --dom-preprune=false disables pruning
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dom_pruning_disabled_keeps_hidden_content_visible_to_extractor() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HIDDEN_ELEMENT_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--dom-preprune=false")
        .arg("--quiet")
        .assert()
        .success();

    // With pruning disabled, the extraction still succeeds.
    let content = t.read_md_content();
    assert!(
        content.contains("visible paragraph"),
        "visible content must be preserved even with pruning disabled"
    );
}

// ---------------------------------------------------------------------------
// --help shows the flag
// ---------------------------------------------------------------------------

#[test]
fn help_contains_dom_preprune_flag() {
    crate::cmd()
        .arg("--help")
        .assert()
        .stdout(predicate::str::contains("--dom-preprune"));
}
