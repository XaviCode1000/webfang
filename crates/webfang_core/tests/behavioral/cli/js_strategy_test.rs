//! Behavioral test: `--js-strategy static` exercises the FetchRouter path.
//!
//! Verifies that the CLI scrape flow wires the JsStrategy into the
//! FetchRouter and produces correct output against a wiremock server (#303).

use crate::cmd;
use crate::BehavioralTest;
use wiremock::matchers::{method, path};
use wiremock::{Mock, ResponseTemplate};

const SEED_HTML: &str = r#"
<html><head><title>JS Strategy Test</title></head>
<body><main><article>
<h1>Static Strategy Page</h1>
<p>This content is served via the static wreq downloader path, confirming
that the FetchRouter wiring dispatches correctly for --js-strategy static.</p>
<p>A second paragraph gives readability enough signal to extract the
document body as the primary content region of this test page.</p>
</article></main></body></html>
"#;

#[tokio::test]
async fn js_strategy_static_scrapes_successfully() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(SEED_HTML)
                .insert_header("Content-Type", "text/html"),
        )
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--quiet")
        .arg("--js-strategy")
        .arg("static")
        .arg("--timeout-secs")
        .arg("5")
        .assert()
        .success();

    let content = t.read_md_content();
    crate::assert_snapshot_redacted(
        "js_strategy_static_scrapes_successfully",
        t.out.path(),
        content,
    );
}

/// Invalid `--js-strategy` values must be rejected at arg-parse time (value
/// enum), not silently defaulted — a typo like `bogus` should fail fast with a
/// non-zero exit and a message naming the valid values (#542 coverage
/// extension). Pure CLI parse test, no network.
#[test]
fn js_strategy_invalid_value_is_rejected() {
    let output = cmd()
        .arg("--url")
        .arg("https://example.com")
        .arg("--js-strategy")
        .arg("bogus")
        .output()
        .expect("run binary");

    assert!(
        !output.status.success(),
        "expected non-zero exit for invalid --js-strategy value"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("js-strategy"),
        "stderr should name the offending flag (--js-strategy): {stderr}"
    );
}
