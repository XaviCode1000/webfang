//! Scraping tools behavioral coverage (issue #450).
//!
//! End-to-end tests for scraping tools not covered by the existing suite:
//! - `scrape_url`: invalid-URL invalid-params (-32602) + HTTP error path
//! - `discover_urls`: real link extraction (internal + external)
//! - `detect_spa`: SPA-marker detection vs. sufficient-content pass vs.
//!   scrape-verdict parity on a JS shell (#760)
//! - `scrape_batch`: partial results when some URLs fail
//! - `crawl_site`: BFS with max_depth=0 (seed only) and max_depth=1 (follows
//!   internal links)
//!
//! All HTTP is wiremock (no real network); all time/randomness is injected.
//!
//! **SSRf Note**: These tests use wiremock on 127.0.0.1 which SSRF blocks.
//! Run with: `WEBFANG_MCP_DISABLE_SSRF=1 cargo nextest run -p webfang_mcp --features mcp --test scraping_coverage_test`

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wreq::Client;

use webfang_core::config::Config;
use webfang_core::di::Container;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::server::ServerOptions;
use webfang_mcp::mcp_server::state::McpState;

/// Minimal article HTML that Readability extracts deterministically (mirrors
/// mcp_behavioral_test.rs), so scraped pages produce real content.
const ARTICLE_HTML: &str = r#"<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
<article>
<h1>Main Heading</h1>
<p>This is the content of the article. It has enough text to be extracted by Readability.</p>
</article>
</body>
</html>"#;

/// A long paragraph whose extracted text exceeds MIN_CONTENT_CHARS (50).
const SUFFICIENT_HTML: &str = r#"<!DOCTYPE html>
<html>
<body>
<p>This is a long paragraph with plenty of substantive content that definitely exceeds the fifty character threshold set by the SPA detector comfortably.</p>
</body>
</html>"#;

/// Initialize SSRF disable flag for tests (idempotent).
fn init_ssrf_disabled() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // Process-wide, permanent setup (no restore-on-drop), so this uses
        // `env_lock` directly — but the mutation is still serialized under
        // the workspace ENV_LOCK invariant (issue #1126).
        let _lock = webfang_test_utils::env_lock();
        std::env::set_var("WEBFANG_MCP_DISABLE_SSRF", "1");
    });
}

// ============================================================================
// Harness helpers — local copies (each integration test binary is standalone).
// ============================================================================

async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    // Disable SSRF before building the router
    init_ssrf_disabled();

    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed");
    let state = McpState::new(container);
    let app = build_mcp_router(state, &ServerOptions::default());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    for _ in 0..20 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    (base_url, handle)
}

fn mcp_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
}

fn extract_json(body: &str) -> Option<Value> {
    if body.contains("data: ") {
        body.lines()
            .filter(|line| line.starts_with("data: "))
            .filter_map(|line| {
                let json_str = line.strip_prefix("data: ").unwrap_or(line);
                serde_json::from_str::<Value>(json_str).ok()
            })
            .next()
    } else {
        serde_json::from_str::<Value>(body).ok()
    }
}

async fn init_session(client: &Client, base_url: &str) -> String {
    let init_body = mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "scraping-coverage-test", "version": "1.0.0" }
        }),
    );
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .json(&init_body)
        .send()
        .await
        .expect("initialize should succeed");
    let session_id = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("initialize must return mcp-session-id");

    let _ = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session_id)
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await;

    session_id
}

async fn call_tool(
    client: &Client,
    base_url: &str,
    session_id: &str,
    name: &str,
    args: Value,
) -> Value {
    let body = mcp_request("tools/call", json!({ "name": name, "arguments": args }));
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("mcp-session-id", session_id)
        .json(&body)
        .send()
        .await
        .expect("tools/call should succeed");
    let text = resp.text().await.expect("read response body");
    extract_json(&text).expect("response must parse as JSON-RPC")
}

fn tool_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .and_then(|first| first.get("text"))
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_string()
}

fn is_tool_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Call a single-URL tool against `target_uri` and return the extracted
/// `result` object from the JSON-RPC envelope.
async fn call_single_url_tool_result(
    client: &Client,
    base_url: &str,
    session_id: &str,
    tool: &str,
    target_uri: &str,
) -> Value {
    let resp = call_tool(
        client,
        base_url,
        session_id,
        tool,
        json!({ "url": target_uri }),
    )
    .await;
    resp.get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone()
}

/// Mount a static page served with a `200` response (shared page fixture).
async fn mount_page_200(mock: &MockServer, route: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(mock)
        .await;
}

/// Run ONE tool call against a fresh MCP server, assert it succeeded, and
/// return the parsed JSON payload of its text result. This file's crawl tests
/// share this prologue as their single canonical copy.
async fn crawl_tool_parsed(tool: &str, params: Value) -> Value {
    let (base_url, _server) = start_test_server().await;
    let client = Client::new();
    let session = init_session(&client, &base_url).await;
    let resp = call_tool(&client, &base_url, &session, tool, params).await;
    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "{tool} should succeed: {}",
        tool_text(&result)
    );
    serde_json::from_str(&tool_text(&result)).expect("tool result must be valid JSON")
}

// ============================================================================
// scrape_url
// ============================================================================

/// A malformed URL is rejected before any fetch with a JSON-RPC
/// invalid-params error (-32602).
#[tokio::test]
async fn test_scrape_url_invalid_url_is_invalid_params() {
    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": "not a valid url" }),
    )
    .await;

    // #1116: the invalid URL is rejected at the `McpUrl` deserialization
    // boundary. rmcp 1.8.0 surfaces argument-deserialization failures as a
    // tool-level error (isError:true); a JSON-RPC -32602 is also accepted.
    // The invariant: the call is rejected and never reaches a fetch.
    let rejected_as_protocol_error = resp
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
        == Some(-32602);
    let rejected_as_tool_error = resp.get("result").map(is_tool_error).unwrap_or(false);
    assert!(
        rejected_as_protocol_error || rejected_as_tool_error,
        "invalid URL must be rejected (protocol -32602 or tool isError), got: {resp}"
    );
}

/// An HTTP 500 from the server surfaces as an honest `CallToolResult::error`
/// (isError:true) — never a fake success.
#[tokio::test]
async fn test_scrape_url_http_error_is_honest_error() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let result =
        call_single_url_tool_result(&client, &base_url, &session_id, "scrape_url", &mock.uri())
            .await;
    assert!(
        is_tool_error(&result),
        "HTTP 500 must return isError:true, got: {}",
        tool_text(&result)
    );
}

/// A JS-shell page (`<div id="app">` mount point, <50 chars of extractable
/// text, spec MCP-1 scenario) must surface as an honest tool error
/// (isError:true) — never a fake `isError:false` success with near-empty
/// content. The underlying `ScrapeError::ExtractionFailed` flows through the
/// existing Err→`CallToolResult::error` mapping (#694, #706).
#[tokio::test]
async fn test_scrape_url_js_shell_is_error_result() {
    let mock = MockServer::start().await;
    // Deterministic JS shell: app mount point + Next.js payload, well under
    // the 50-char content threshold once readability/fallback strips markup.
    let html = r#"<!DOCTYPE html>
<html><head><title>App</title>
<script id="__NEXT_DATA__" type="application/json">{"page":"/"}</script>
</head><body><div id="app"></div></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let result =
        call_single_url_tool_result(&client, &base_url, &session_id, "scrape_url", &mock.uri())
            .await;
    assert!(
        is_tool_error(&result),
        "a JS-shell scrape must return isError:true, got: {}",
        tool_text(&result)
    );

    // Semantic invariant: the Spanish error text names the JS-rendering cause.
    let text = tool_text(&result);
    assert!(
        text.contains("renderizado de JavaScript"),
        "error text must explain the JS-rendering cause, got: {text}"
    );

    // Redacted snapshot (XC-2): the wiremock port is the only
    // non-deterministic element — collapse it before snapshotting.
    let redacted = text.replace(&mock.uri(), "http://127.0.0.1:<PORT>");
    insta::assert_snapshot!("scrape_url_js_shell_is_error", redacted);
}

// ============================================================================
// discover_urls
// ============================================================================

/// Link extraction resolves relative links against the page URL and keeps
/// external links; all three links are returned as absolute URLs in document
/// order.
#[tokio::test]
async fn test_discover_urls_extracts_internal_and_external_links() {
    let mock = MockServer::start().await;
    let html = r#"<html><body>
<a href="/page1">Page 1</a>
<a href="/page2">Page 2</a>
<a href="https://other.example.com/foo">External</a>
</body></html>"#;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "discover_urls",
        json!({ "url": mock.uri() }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "discover_urls should succeed: {}",
        tool_text(&result)
    );

    let links: Vec<String> =
        serde_json::from_str(&tool_text(&result)).expect("discovered links must be a JSON array");
    assert_eq!(
        links,
        vec![
            format!("{}/page1", mock.uri()),
            format!("{}/page2", mock.uri()),
            "https://other.example.com/foo".to_string(),
        ],
        "links must be absolute, deduped, and in document order"
    );
}

// ============================================================================
// detect_spa
// ============================================================================

/// A short body with a `<div id="root">` mount point is flagged as an SPA with
/// `has_spa_markers: true`.
#[tokio::test]
async fn test_detect_spa_short_content_with_root_marker() {
    let mock = MockServer::start().await;
    let html = r#"<html><body><div id="root"></div></body></html>"#;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_spa",
        json!({ "url": mock.uri() }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "detect_spa should succeed: {}",
        tool_text(&result)
    );

    let parsed: Value =
        serde_json::from_str(&tool_text(&result)).expect("SPA result must be valid JSON");
    let reported_url = parsed.get("url").and_then(|v| v.as_str());
    assert!(
        reported_url.is_some(),
        "SPA result must report a URL, got: {parsed}"
    );
    let mock_uri = mock.uri();
    let url_str = reported_url.unwrap();
    // Mock URI normalization: wiremock URI may have trailing slash or not
    assert!(
        url_str == mock_uri.as_str() || url_str.trim_end_matches('/') == mock_uri.as_str(),
        "SPA result must report the analyzed URL, expected: {mock_uri}, got: {url_str}"
    );
    assert_eq!(
        parsed.get("has_spa_markers").and_then(|v| v.as_bool()),
        Some(true),
        "root mount point must set the SPA marker"
    );
    let char_count = parsed
        .get("char_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::MAX);
    assert!(
        char_count < 50,
        "extracted content must be below MIN_CONTENT_CHARS, got: {parsed}"
    );
}

/// A body with substantial text (> MIN_CONTENT_CHARS) is NOT an SPA.
#[tokio::test]
async fn test_detect_spa_sufficient_content_not_spa() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(SUFFICIENT_HTML))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "detect_spa",
        json!({ "url": mock.uri() }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "detect_spa should succeed: {}",
        tool_text(&result)
    );
    assert_eq!(tool_text(&result), "not an SPA - sufficient content found");
}

/// #760 parity: one JS-shell fixture (app mount point + a long `<noscript>`
/// block that defeated the pre-fix detector — raw htmd counted the noscript
/// text, clean-based extraction does not) must produce IDENTICAL verdicts on
/// both surfaces: `detect_spa` reports an SPA (JSON, markers true) and
/// `scrape_url` fails honestly with the JavaScript-rendering error. The tool
/// signal now PREDICTS the scrape's behavior instead of contradicting it.
#[tokio::test]
async fn test_detect_spa_predicts_scrape_verdict_on_js_shell() {
    let mock = MockServer::start().await;
    // Shell whose only text lives inside <noscript>: enough (200+ chars) to
    // clear MIN_CONTENT_CHARS on the old raw-htmd detector, near-zero once the
    // real pipeline's cleaning runs.
    let html = format!(
        "<!DOCTYPE html><html><head><title>JS App</title></head><body>\
         <div id=\"app\"></div>\
         <noscript>{}</noscript>\
         </body></html>",
        "JavaScript must be enabled to view this quoting application. ".repeat(4)
    );
    mount_page_200(&mock, "/", &html).await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    // 1. detect_spa must predict the SPA verdict (JSON with markers), not the
    //    pre-fix "not an SPA - sufficient content found" literal.
    let result =
        call_single_url_tool_result(&client, &base_url, &session_id, "detect_spa", &mock.uri())
            .await;
    assert!(
        !is_tool_error(&result),
        "detect_spa should succeed: {}",
        tool_text(&result)
    );
    let text = tool_text(&result);
    assert_ne!(
        text, "not an SPA - sufficient content found",
        "the tool must no longer contradict the scrape on a JS shell"
    );
    let parsed: Value = serde_json::from_str(&text).expect("SPA result must be valid JSON");
    assert_eq!(
        parsed.get("has_spa_markers").and_then(|v| v.as_bool()),
        Some(true),
        "the app mount point must be reported: {parsed}"
    );
    let char_count = parsed
        .get("char_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::MAX);
    assert!(
        char_count < 50,
        "extracted content after cleaning must be below the threshold: {parsed}"
    );

    // 2. scrape_url on the SAME fixture must fail with the JS-rendering
    //    verdict the tool just predicted.
    let scrape_result =
        call_single_url_tool_result(&client, &base_url, &session_id, "scrape_url", &mock.uri())
            .await;
    assert!(
        is_tool_error(&scrape_result),
        "the scrape must fail the minimum-content guard the tool predicted: {}",
        tool_text(&scrape_result)
    );
    assert!(
        tool_text(&scrape_result).contains("renderizado de JavaScript"),
        "the scrape error must name the JS cause: {}",
        tool_text(&scrape_result)
    );

    // Snapshot (XC-2): redact the wiremock port for determinism.
    let redacted = text.replace(&mock.uri(), "http://127.0.0.1:<PORT>");
    insta::assert_snapshot!("detect_spa_js_shell_predicts_scrape", redacted);
}

// ============================================================================
// scrape_batch
// ============================================================================

/// Failed URLs are logged but do not stop the batch: 2 OK + 1 error → a
/// successful result with exactly the 2 scraped pages + the failed URL in the
/// `failed` array (issue #591).
#[tokio::test]
async fn test_scrape_batch_partial_results_on_failure() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(ARTICLE_HTML))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/c"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_batch",
        json!({
            "urls": [format!("{}/a", mock.uri()), format!("{}/b", mock.uri()), format!("{}/c", mock.uri())]
        }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "batch with partial failures must still succeed, got: {}",
        tool_text(&result)
    );

    let text = tool_text(&result);
    // Issue #591: response is now { results: [...], failed: [...] }
    let outcome: Value = serde_json::from_str(&text).expect("batch result must be valid JSON");
    let pages = outcome
        .get("results")
        .and_then(|v| v.as_array())
        .expect("batch result must have a 'results' array");
    assert_eq!(
        pages.len(),
        2,
        "only the 2 successful pages should be returned, got: {text}"
    );

    let failed = outcome
        .get("failed")
        .and_then(|v| v.as_array())
        .expect("batch result must have a 'failed' array");
    assert_eq!(
        failed.len(),
        1,
        "one URL should be in the failed array, got: {text}"
    );
    assert_eq!(
        failed[0]["url"],
        format!("{}/c", mock.uri()),
        "failed URL must be /c, got: {text}"
    );
    assert!(
        failed[0]["error"].as_str().unwrap().contains("500"),
        "error message should mention 500, got: {text}"
    );
}

// ============================================================================
// crawl_site
// ============================================================================

/// max_depth=0 crawls only the seed page: no link following, so the result
/// contains exactly one URL and zero errors.
#[tokio::test]
async fn test_crawl_site_max_depth_zero_single_page() {
    let mock = MockServer::start().await;
    let html = r#"<html><body>
<a href="/page_a">A</a>
<a href="/page_b">B</a>
</body></html>"#;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&mock)
        .await;

    let (base_url, _handle) = start_test_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "crawl_site",
        json!({ "url": mock.uri(), "max_depth": 0 }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "crawl_site should succeed: {}",
        tool_text(&result)
    );

    let parsed: Value =
        serde_json::from_str(&tool_text(&result)).expect("crawl result must be valid JSON");
    assert_eq!(
        parsed.get("total_pages").and_then(|v| v.as_u64()),
        Some(1),
        "max_depth=0 must crawl only the seed, got: {parsed}"
    );
    let urls = parsed
        .get("urls")
        .and_then(|v| v.as_array())
        .expect("urls array present");
    assert_eq!(urls.len(), 1, "exactly one URL expected, got: {parsed}");
    assert_eq!(
        urls[0].as_str(),
        Some(format!("{}/", mock.uri()).as_str()),
        "the single URL must be the seed (root serialized with trailing slash), got: {parsed}"
    );
    assert_eq!(
        parsed.get("errors").and_then(|v| v.as_u64()),
        Some(0),
        "no crawl errors expected, got: {parsed}"
    );
}

/// max_depth=1 follows internal links one level: seed + page_a + page_b, with
/// zero errors. The `urls` array is order-independent (JoinSet concurrency).
#[tokio::test]
async fn test_crawl_site_max_depth_one_follows_internal_links() {
    let mock = MockServer::start().await;
    let index_html = r#"<html><body>
<a href="/page_a">A</a>
<a href="/page_b">B</a>
</body></html>"#;
    mount_page_200(&mock, "/", index_html).await;
    mount_page_200(&mock, "/page_a", "<html><body>Page A</body></html>").await;
    mount_page_200(&mock, "/page_b", "<html><body>Page B</body></html>").await;

    let parsed =
        crawl_tool_parsed("crawl_site", json!({ "url": mock.uri(), "max_depth": 1 })).await;
    assert_eq!(
        parsed.get("total_pages").and_then(|v| v.as_u64()),
        Some(3),
        "max_depth=1 must crawl seed + 2 linked pages, got: {parsed}"
    );
    let urls: Vec<String> = parsed
        .get("urls")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|u| u.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let mut expected = vec![
        format!("{}/", mock.uri()),
        format!("{}/page_a", mock.uri()),
        format!("{}/page_b", mock.uri()),
    ];
    expected.sort();
    let mut got = urls.clone();
    got.sort();
    assert_eq!(got, expected, "crawled URL set mismatch, got: {urls:?}");
    assert_eq!(
        parsed.get("errors").and_then(|v| v.as_u64()),
        Some(0),
        "no crawl errors expected, got: {parsed}"
    );
}

/// REQ-01 (SSRF defense-in-depth): `crawl_site` output contains only
/// seed-host URLs — an external-domain link in the seed page never reaches
/// the response `urls` array. Note: the crawl engine ALSO gates internal
/// links (the external URL is neither crawled nor surfaced there); this test
/// documents both layers at once, and the MCP post-hoc filter is the
/// authoritative gate for non-crawl discovery paths (sitemap, see below).
#[tokio::test]
async fn test_crawl_site_output_excludes_external_links() {
    let mock = MockServer::start().await;
    let index_html = r#"<html><body>
<a href="/page_a">A</a>
<a href="https://external.example/x">External</a>
</body></html>"#;
    mount_page_200(&mock, "/", index_html).await;
    mount_page_200(&mock, "/page_a", "<html><body>Page A</body></html>").await;

    let parsed =
        crawl_tool_parsed("crawl_site", json!({ "url": mock.uri(), "max_depth": 1 })).await;
    let urls = parsed
        .get("urls")
        .and_then(|v| v.as_array())
        .expect("urls array present");
    assert!(
        !urls.is_empty(),
        "crawl must discover at least the seed, got: {parsed}"
    );

    // The engine internally gates non-internal links too (defense-in-depth),
    // so this assert is the union of both layers: ONLY seed-host URLs surface.
    let seed_host = url::Url::parse(&mock.uri())
        .expect("mock URI parses")
        .host_str()
        .expect("mock URI has host")
        .to_string();
    let mut got: Vec<String> = urls
        .iter()
        .filter_map(|u| u.as_str().map(String::from))
        .collect();
    got.sort();
    let mut expected = vec![format!("{}/", mock.uri()), format!("{}/page_a", mock.uri())];
    expected.sort();
    assert_eq!(
        got, expected,
        "urls array must contain only seed-host ({seed_host}) URLs, got: {got:?}"
    );
    let has_external = urls
        .iter()
        .filter_map(|u| u.as_str())
        .any(|u| u.contains("external.example"));
    assert!(
        !has_external,
        "external-domain URL must never reach the response, got: {parsed}"
    );
}

/// REQ-01 (SSRF defense-in-depth): `crawl_with_sitemap` output contains only
/// seed-host URLs. Sitemap discovery returns every `<loc>` host-agnostic (the
/// engine does NOT apply internal-link gating there), so this test exercises
/// the MCP post-hoc filter as the authoritative gate: an external-domain URL
/// and a forbidden-literal-IP URL from the sitemap must both be excluded.
/// Fully offline: only the sitemap XML is fetched (wiremock on the seed
/// host); the listed pages are never fetched (discovery-only crawl).
#[tokio::test]
async fn test_crawl_with_sitemap_response_excludes_external_and_forbidden_urls() {
    let mock = MockServer::start().await;

    let sitemap = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>{base}/internal</loc></url>
  <url><loc>https://external.example/x</loc></url>
  <url><loc>http://10.0.0.5/forbidden</loc></url>
</urlset>"#,
        base = mock.uri(),
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/xml")
                .set_body_string(sitemap),
        )
        .mount(&mock)
        .await;

    let parsed = crawl_tool_parsed(
        "crawl_with_sitemap",
        json!({
            "url": mock.uri(),
            "sitemap_url": format!("{}/sitemap.xml", mock.uri()),
        }),
    )
    .await;

    let urls: Vec<String> =
        serde_json::from_value(parsed).expect("sitemap result must be a JSON array");

    let seed_host = url::Url::parse(&mock.uri())
        .expect("mock URI parses")
        .host_str()
        .expect("mock URI has host")
        .to_string();

    assert!(
        !urls.is_empty(),
        "the internal seed-host URL must survive the filter, got: {urls:?}"
    );
    let mut got = urls.clone();
    got.sort();
    assert_eq!(
        got,
        vec![format!("{}/internal", mock.uri())],
        "only the seed-host ({seed_host}) URL may surface, got: {got:?}"
    );
    assert!(
        !urls.iter().any(|u| u.contains("external.example")),
        "external-domain sitemap URLs must be excluded, got: {urls:?}"
    );
    assert!(
        !urls.iter().any(|u| u.contains("10.0.0.5")),
        "forbidden-literal-IP sitemap URLs must be excluded, got: {urls:?}"
    );
}
