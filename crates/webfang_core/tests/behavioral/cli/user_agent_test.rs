//! `--user-agent` pinning: the CLI flag reaches the wire (#503).
//!
//! Before the fix, `--user-agent` stopped at `HttpClientConfig` and never
//! reached the wreq layer — the server saw the emulation-profile default UA.
//! The mock below answers ONLY when the request carries exactly the pinned
//! UA, so a successful scrape proves the flag is on the wire end-to-end.

use crate::cmd;
use crate::BehavioralTest;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, ResponseTemplate};

const SEED_HTML: &str = r#"
<html><head><title>User Agent Pin Test</title></head>
<body><main><article>
<h1>Pinned Identity</h1>
<p>This content is only served when the pinned User-Agent arrives on the wire.</p>
<p>More paragraphs ensure readability can extract a proper document.</p>
</article></main></body></html>
"#;

#[tokio::test]
async fn user_agent_flag_reaches_the_wire() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("User-Agent", "QA-Bot/9.9"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .expect(1)
        .mount(&t.server)
        .await;

    t.scraper_cmd()
        .arg("--single-page")
        .arg("--user-agent")
        .arg("QA-Bot/9.9")
        .arg("--quiet")
        .assert()
        .success();

    let content = t.read_md_content();
    crate::assert_snapshot_redacted("user_agent_flag_reaches_the_wire", t.out.path(), content);
}

/// Negative complement of `user_agent_flag_reaches_the_wire`: the EXACT pinned
/// UA must travel on the wire. The mock only answers to `Pinned/1.0`; we send
/// `Wrong/2.0` and assert the received request carried exactly that value —
/// proving the flag controls the wire UA and is not silently replaced by a
/// default (#542 coverage extension). No real network.
#[tokio::test]
async fn user_agent_pinned_value_is_transmitted_exactly() {
    let t = BehavioralTest::new().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .and(header("User-Agent", "Pinned/1.0"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SEED_HTML))
        .mount(&t.server)
        .await;

    let output = cmd()
        .arg("--url")
        .arg(t.server.uri())
        .arg("--single-page")
        .arg("--user-agent")
        .arg("Wrong/2.0")
        .arg("--output")
        .arg(t.out.path())
        .arg("--quiet")
        .output()
        .expect("run binary");

    let requests = t.server.received_requests().await.unwrap();
    assert!(
        !requests.is_empty(),
        "expected at least one request to the mock server"
    );

    // Inspect only the actual page request (path "/"); preflight/robots/favicon
    // requests may legitimately carry a different UA.
    let page_request = requests
        .iter()
        .find(|r| r.url.path() == "/")
        .expect("expected a request to the seed path /");
    let ua = page_request
        .headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok().map(|s| s.to_string()))
        .expect("page request carried a User-Agent header");

    assert_eq!(
        ua, "Wrong/2.0",
        "the exact pinned --user-agent value must be transmitted on the wire"
    );
    assert_ne!(
        ua, "Pinned/1.0",
        "the pinned UA must NOT be the value the mock reserved for the success case"
    );

    // The crawl could not have succeeded content-wise (mock only answered the
    // other UA), so no markdown should have been produced.
    assert!(
        t.find_files("md").is_empty(),
        "expected no .md output because the pinned UA did not match the mock"
    );
    let _ = output;
}
