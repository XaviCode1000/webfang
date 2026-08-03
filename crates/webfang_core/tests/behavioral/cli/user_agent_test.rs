//! `--user-agent` pinning: the CLI flag reaches the wire (#503).
//!
//! Before the fix, `--user-agent` stopped at `HttpClientConfig` and never
//! reached the wreq layer — the server saw the emulation-profile default UA.
//! The mock below answers ONLY when the request carries exactly the pinned
//! UA, so a successful scrape proves the flag is on the wire end-to-end.

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
