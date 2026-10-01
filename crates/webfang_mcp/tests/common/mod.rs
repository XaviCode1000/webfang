//! Shared MCP test harness — centralized server bootstrap + JSON-RPC helpers.
//!
//! Each integration test binary is standalone, so every test file that needs a
//! live MCP server previously duplicated ~70 lines of bootstrap (container +
//! state + axum serve + session helpers). This module centralizes that setup.
//!
//! Include it with `mod common;` (or `mod common; use common::*;`) at the top of
//! a test file, then call `common::start_test_server()` and friends. The helpers
//! are intentionally `pub` but only a subset is used by any given binary, so the
//! module carries `#![allow(dead_code)]`.
//!
//! **Declare it bare — no `#[path]`.** `mod common;` already resolves to this
//! exact file (`tests/common/mod.rs`), so `#[path = "common/mod.rs"]` would be a
//! no-op that restates the compiler's own resolution. This crate settled the
//! bare form deliberately when #1598 migrated the last three stragglers off
//! their local harness copies: all 15 test targets here declare a bare
//! `mod common;`, none spells out `#[path]`, and the three that previously
//! carried one were the minority, not the convention. Note this is the
//! *webfang_mcp* convention — `webfang_core`'s `tests/common/mod.rs` is reached
//! as `common::cli_harness::{...}` (a nested submodule) and 20 of its 24
//! targets do spell out `#[path]`; the two crates' harnesses are not
//! interchangeable, so do not copy one crate's header shape into the other.
//!
//! **Leave the `use common::{...}` line on one line.** `rustfmt.toml` sets
//! `max_width = 100` and no `imports_width`/`imports_layout` override, so
//! rustfmt's `Mixed` layout keeps a nested import list on a single line
//! whenever it fits in `max_width` — and it *rewrites* a hand-wrap back.
//! The longest such line here (`start_test_server_ssrf_enabled` in
//! security_tools_test.rs / obsidian_tools_test.rs) measures 96 chars. Only
//! past 100 does the wrap become mandatory, as in
//! sitemap_crawl_run_staleness_test.rs:40, whose single-line form would be
//! 102 chars.
//!
//! Page fixtures live here too: [`mount_page_200`] is the ONE canonical way an
//! MCP test mounts an HTML page (#1371) — do not hand-roll a second one.
//!
//! **SSRF Note**: `start_test_server()` and related functions disarm BOTH
//! wiremock-loopback hatches before building the router: the MCP entry
//! pre-check (`WEBFANG_MCP_DISABLE_SSRF=1`) and the literal-IP filter in the
//! asset download chain (`DISABLE_ENTRY_GUARD_ENV=1`, PI-1/SEC F1). See
//! [`arm_wiremock_hatches`].
//! This is required because wiremock uses 127.0.0.1 for its mock HTTP server,
//! which SSRF protection blocks by design. The single exception is
//! `start_test_server_ssrf_enabled()`, which intentionally leaves the guard ON
//! to exercise it against forbidden addresses (issue #703 integration probe).

#![allow(dead_code)]

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

/// Arm BOTH wiremock-loopback hatches, idempotently: the MCP entry pre-check
/// (layer 1, `WEBFANG_MCP_DISABLE_SSRF`) AND the literal-IP filter the asset
/// download chain enforces at `download_asset_urls` (layer 2,
/// `DISABLE_ENTRY_GUARD_ENV`, PI-1/SEC F1). wiremock binds 127.0.0.1 — a
/// forbidden production literal — so asset URLs extracted from mock HTML are
/// now rejected by layer 2 unless it is disarmed too.
///
/// Process-wide, permanent setup (no restore-on-drop): `env_set` acquires
/// ENV_LOCK itself, so the mutation stays serialized under the workspace
/// ENV_LOCK invariant (issue #1126) without a manual `env_lock` binding.
pub fn arm_wiremock_hatches() {
    webfang_test_utils::env_set(
        webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
        "1",
    );
    webfang_test_utils::env_set(
        webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
        "1",
    );
}

/// Initialize SSRF disable flags for tests (idempotent).
fn init_ssrf_disabled() {
    arm_wiremock_hatches();
}

/// Bind `app` to a random 127.0.0.1 port, serve it, and wait until it accepts
/// connections. Returns the base URL and the server handle.
///
/// This is the shared tail of every server starter in this module — do not
/// duplicate the bind/serve/wait sequence.
pub async fn serve_on_random_port(app: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let base_url = format!("http://{addr}");

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Wait for the server to accept TCP connections instead of a fixed sleep.
    for _ in 0..20 {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    (base_url, handle)
}

/// Ephemeral DI container over the default test configuration. NOTE:
/// `Container::new` creates real HTTP clients (wreq) and a real service layer.
/// This is intentional for integration tests — the container is ephemeral and
/// scoped to the test, so real infrastructure gives confidence that the MCP
/// server works end-to-end with the actual application state.
async fn test_container() -> Container {
    let config = Config::default();
    Container::new(config.crawler, config.scraper)
        .await
        .expect("container creation failed")
}

/// The token-less test preset (#1611 G-18): authentication is fail-closed by
/// default, so every harness that drives a token-less router states the
/// opt-in once, here. Production gets the same choice from `--allow-anonymous`.
pub fn anonymous_server_options() -> ServerOptions {
    ServerOptions {
        allow_anonymous: true,
        ..Default::default()
    }
}

/// Start the real MCP router with explicit [`ServerOptions`] on a random
/// loopback port. The general form every starter in this module presets —
/// call it directly when a suite needs a token, a quota, or any other
/// non-default option.
pub async fn start_server_with_options(
    options: ServerOptions,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = build_mcp_router(McpState::new(test_container().await), &options);
    serve_on_random_port(app).await
}

/// Start a test MCP server on a random port and return the base URL.
pub async fn start_test_server() -> (String, tokio::task::JoinHandle<()>) {
    // Disable SSRF protection for tests (uses 127.0.0.1 for wiremock)
    init_ssrf_disabled();

    start_server_with_options(anonymous_server_options()).await
}

/// Start a test MCP server on a random port. Pass `Some(downloader)` to inject
/// a shared `AssetDownloaderPort` (the production wiring in `mcp_server.rs`);
/// pass `None` to exercise the per-call fallback downloader built from config.
pub async fn start_server(
    downloader: Option<std::sync::Arc<webfang_core::adapters::downloader::Downloader>>,
) -> (String, tokio::task::JoinHandle<()>) {
    // Disable BOTH SSRF hatches for tests (wiremock binds 127.0.0.1).
    // Permanent set (no restore-on-drop) — see `arm_wiremock_hatches` for the
    // layer-2 rationale and the ENV_LOCK serialization (issue #1126).
    arm_wiremock_hatches();

    let container = test_container().await;
    let state = match downloader {
        Some(d) => McpState::new(container).with_downloader(d),
        None => McpState::new(container),
    };

    let app = build_mcp_router(state, &anonymous_server_options());

    serve_on_random_port(app).await
}

/// Start a test MCP server whose crawl-result repository is pre-seeded with
/// `n` `ScrapedContent` items.
///
/// Returns `(base_url, server_handle, container_tmp)`. The container temp dir
/// is returned so the caller keeps it alive (the append-only repository log
/// lives inside it) and so tests can locate exports that default to the
/// container's configured `output_dir` (e.g. `process_export_pipeline`).
pub async fn start_seeded_server(
    n: usize,
) -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    use webfang_core::domain::config::ScraperConfig;
    use webfang_core::domain::{CrawlerConfig, ScrapedContent, ValidUrl};

    let container_tmp = tempfile::TempDir::new().expect("create container temp dir");
    let crawler_config =
        CrawlerConfig::new(url::Url::parse("https://seed.example.com").expect("valid seed URL"));
    let scraper_config = ScraperConfig {
        output_dir: container_tmp.path().to_path_buf(),
        ..Default::default()
    };
    let container = Container::new(crawler_config, scraper_config)
        .await
        .expect("container creation failed");

    // Seed the crawl-result repository with n items and wait for indexing.
    let repo = container
        .crawl_result_repository()
        .expect("container must wire a crawl result repository");
    for i in 0..n {
        let url_str = format!("https://seed.example.com/page/{i}");
        let url = url::Url::parse(&url_str).expect("valid seeded URL");
        let content = ScrapedContent {
            title: format!("Seed Title {i}"),
            content: format!("Seed body content number {i} for export testing."),
            url: ValidUrl::try_from_url(url).expect("seeded fixture is a plain https URL"),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: vec![],
            correlation_id: None,
            quality_hint: None,
        };
        repo.save(&content).expect("save seeded content");
    }
    // Poll until the background writer has indexed every seeded URL.
    for i in 0..n {
        let url_str = format!("https://seed.example.com/page/{i}");
        for _ in 0..80 {
            if repo.find_by_url(&url_str).expect("find_by_url").is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    // The pipeline's write target is the container's configured output_dir;
    // process_export_pipeline enforces the export-root gate at request time
    // (XP-P-08/G-9, issue #1608), so the fixture declares the container temp
    // dir as a root — the shape of a deployment whose --export-roots covers
    // the configured output directory.
    let state =
        McpState::new(container).with_export_roots(vec![container_tmp.path().to_path_buf()]);

    // Disable BOTH SSRF hatches for tests (uses 127.0.0.1 for wiremock).
    // Permanent set serialized under ENV_LOCK by `env_set` — see
    // `arm_wiremock_hatches` (issue #1126).
    arm_wiremock_hatches();

    let app = build_mcp_router(state, &anonymous_server_options());

    let (base_url, handle) = serve_on_random_port(app).await;

    (base_url, handle, container_tmp)
}

/// Start a test MCP server with SSRF protection ENABLED.
///
/// Every other starter sets `WEBFANG_MCP_DISABLE_SSRF=1` because wiremock binds
/// 127.0.0.1; this one exists precisely to exercise the guard against those
/// forbidden addresses (issue #703 integration probe). Tests using it must
/// target IPs that fail validation BEFORE any fetch, so no mock server is
/// needed. Also actively removes the disable flag in case a shared CI/parent
/// environment exported it.
pub async fn start_test_server_ssrf_enabled() -> (String, tokio::task::JoinHandle<()>) {
    // Actively remove both hatches (serialized under ENV_LOCK by
    // `env_remove` — see `arm_wiremock_hatches`) in case a shared CI/parent
    // environment exported them (issue #1126).
    webfang_test_utils::env_remove(webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV);
    webfang_test_utils::env_remove(webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV);

    start_server_with_options(anonymous_server_options()).await
}

/// JSON-RPC 2.0 standard error code for "Invalid params".
///
/// Shared by the suites that assert on rejection envelopes (`params_rejection`,
/// `mcp_ssrf_error_class`, `mcp_error_channel_mapping`) so the constant has one
/// definition instead of one per file.
pub const JSONRPC_INVALID_PARAMS: i64 = -32602;

/// The JSON-RPC `error.code` of a parsed response, or `None` when the response
/// carries no top-level `error` member (the tool-error channel).
pub fn error_code(resp: &Value) -> Option<i64> {
    resp.get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_i64())
}

/// Build a JSON-RPC request body for MCP protocol.
pub fn mcp_request(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    })
}

/// The `Accept` value a spec-compliant MCP client must send: rmcp answers 406
/// to anything naming neither media type.
pub const MCP_ACCEPT: &str = "application/json, text/event-stream";

/// A well-formed `initialize` body — the request that creates a session.
/// `client_name` only identifies the caller in server logs; pass the suite's
/// name.
pub fn initialize_body(client_name: &str) -> Value {
    mcp_request(
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": client_name, "version": "1.0.0" }
        }),
    )
}

/// One POST to `/mcp` with an optional bearer token, returning
/// `(status, retry-after, mcp-session-id)`. A header the response does not
/// carry comes back as `None`, so suites that don't exercise rate limiting
/// or sessions just ignore the slots they don't need.
pub async fn post_mcp(
    client: &Client,
    base_url: &str,
    body: &Value,
    token: Option<&str>,
) -> (u16, Option<String>, Option<String>) {
    let mut request = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT);
    if let Some(token) = token {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let resp = request
        .json(body)
        .send()
        .await
        .expect("request should be sent");
    let status = resp.status().as_u16();
    let header = |name: &str| {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(String::from)
    };
    (status, header("retry-after"), header("mcp-session-id"))
}

/// Extract the first JSON-RPC object from an SSE (`data: ` prefixed) or direct
/// JSON response body.
pub fn extract_json(body: &str) -> Option<Value> {
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

/// Redact a known output directory path so insta snapshots stay stable
/// run-to-run.
pub fn redact_path(text: &str, dir: &std::path::Path) -> String {
    text.replace(dir.to_string_lossy().as_ref(), "[OUT_DIR]")
}

/// Initialize an MCP session (initialize + notifications/initialized) and
/// return the session ID.
pub async fn init_session(client: &Client, base_url: &str) -> String {
    let init_body = initialize_body("export-test");
    let resp = client
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", MCP_ACCEPT)
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
        .header("Accept", MCP_ACCEPT)
        .header("mcp-session-id", &session_id)
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await;

    session_id
}

/// Call an MCP tool and return the parsed JSON-RPC response object.
pub async fn call_tool(
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
        .header("Accept", MCP_ACCEPT)
        .header("mcp-session-id", session_id)
        .json(&body)
        .send()
        .await
        .expect("tools/call should succeed");
    let text = resp.text().await.expect("read response body");
    extract_json(&text).expect("response must parse as JSON-RPC")
}

/// Extract the first content text from a tool result object.
///
/// #1600 provenance-aware: `untrusted_text` responses arrive wrapped in the
/// UNTRUSTED envelope, so the raw tool text is NOT the payload. Strip the
/// envelope when present (errors and `local_text` results are unenveloped)
/// and dedent the one-space indentation `RemoteDerived` bodies carry, so
/// assertions keep comparing against the handler's original payload.
pub fn tool_text(result: &Value) -> String {
    payload_text(
        result
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or_default(),
    )
}

/// Provenance-envelope-aware payload extraction: the payload between the
/// BEGIN/END UNTRUSTED markers when the text is enveloped, the text itself
/// otherwise. See [`unwrap_untrusted`] for the strict variant.
pub fn payload_text(raw: &str) -> String {
    match webfang_mcp::mcp_server::provenance::payload_of(raw) {
        // RemoteDerived bodies keep their one-space indentation (part of the
        // defense, see provenance::indent_lines) — tests that compare exact
        // text against such a payload trim it explicitly.
        Some(payload) => payload.to_string(),
        None => raw.to_string(),
    }
}

/// Whether a tool result is flagged as an error (CallToolResult::error).
pub fn is_tool_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ========================================================================
// Page fixtures — the ONE canonical HTML page mount (#1371, follows #1354)
// ========================================================================

/// The `200` response behind [`mount_page_200`]: the single place the page
/// MIME is decided, for the rare mount that needs its own wiring (an
/// expectation is covered by [`mount_page_200_expect`]; anything else goes
/// through [`mount_page_200`]).
///
/// `text/html` is load-bearing, not cosmetic: wiremock's `set_body_string`
/// answers `text/plain`, and Chrome renders that as an inert source dump —
/// scripts never execute, so a render or settle test over such a fixture
/// asserts on the delivery, never the render path (#1354). Serving HTML as
/// HTML is what lets every plane below (static extraction *and* chromium)
/// see the same document.
pub fn html_page_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body, "text/html")
}

/// Mount a static HTML page at `route`, answered `200` as `text/html` —
/// the canonical page fixture for every MCP test binary (#1371). See
/// [`html_page_response`] for why the MIME is part of the contract.
pub async fn mount_page_200(mock: &MockServer, route: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(html_page_response(body))
        .mount(mock)
        .await;
}

/// [`mount_page_200`] plus a wiremock request-count expectation, for tests
/// whose proof IS the number of hits (`expect(1)` on a fetched page,
/// `expect(0)` on one that must never be reached). Keeping the MIME in the
/// shared helper is the point: the count-only variants were exactly where
/// a `text/plain` HTML fixture crept back in (#1354).
///
/// The count is a `u64` because that is what wiremock's `Times` accepts
/// (`From<u64>`); call sites pass plain literals.
pub async fn mount_page_200_expect(
    mock: &MockServer,
    route: &str,
    body: &str,
    expected_requests: u64,
) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(html_page_response(body))
        .expect(expected_requests)
        .mount(mock)
        .await;
}

/// Provenance-envelope inverse for tests (#1600): most provenance-gated tool
/// responses now arrive wrapped in the UNTRUSTED envelope, so a test that
/// asserts on the payload itself extracts the section between the
/// `---- BEGIN UNTRUSTED <nonce> ----` / `---- END UNTRUSTED <nonce> ----`
/// marker lines first. Delegates to the canonical inverse
/// `webfang_mcp::mcp_server::provenance::payload_of`; panics when the text is
/// NOT enveloped (local_text results and errors need no unwrap).
pub fn unwrap_untrusted(text: &str) -> &str {
    webfang_mcp::mcp_server::provenance::payload_of(text)
        .expect("expected a provenance-enveloped (untrusted_text) result; local results and errors carry no envelope")
}
