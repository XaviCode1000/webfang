//! Scraping tools behavioral coverage (issue #450).
//!
//! End-to-end tests for scraping tools not covered by the existing suite:
//! - `scrape_url`: invalid-URL invalid-params (-32602) + HTTP error path
//! - `discover_urls`: real link extraction (internal + external)
//! - `detect_spa`: SPA-marker detection vs. sufficient-content pass
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

    let error = resp
        .get("error")
        .unwrap_or_else(|| panic!("invalid URL must return a JSON-RPC error, got: {resp}"));
    let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    assert_eq!(
        code, -32602,
        "invalid URL must map to JSON-RPC invalid-params (-32602), got: {error}"
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

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "scrape_url",
        json!({ "url": mock.uri() }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        is_tool_error(&result),
        "HTTP 500 must return isError:true, got: {}",
        tool_text(&result)
    );
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
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(index_html))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/page_a"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>Page A</body></html>"),
        )
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/page_b"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>Page B</body></html>"),
        )
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
        json!({ "url": mock.uri(), "max_depth": 1 }),
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
