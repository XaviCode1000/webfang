//! CLI ↔ MCP JSONL export record parity — the acceptance evidence that
//! closes P6-2/F-16 (#1290).
//!
//! AUDIT-02 §4/§6/§15: the MCP `export_jsonl` used to read server
//! persistence while the CLI exported in-memory results, so the same
//! operation produced different records per surface (INV-005 FAIL). The
//! slice rewires the MCP export path to the session-owned results of the
//! last `crawl_site` run; this test proves the failure criterion on one
//! shared wiremock site:
//!
//!   `webfang <url> --max-depth 1 … --pipeline-format jsonl`  (CLI)
//!   `crawl_site` + `export_jsonl`                             (MCP)
//!
//! must produce record-equivalent JSONL modulo the transport envelope.
//! Equivalence is asserted on the FULL record arrays — `url`, `title`,
//! `content`, `checksum_sha256`, `metadata_version`, `content_length`,
//! `word_count`, `reading_time`, `extra_metadata` — after redacting only
//! `timestamp_utc` (the one per-run field). The crawl completion order is
//! nondeterministic (`buffer_unordered`), so records are compared as
//! url-keyed sets, mirroring `discovery_parity_1232`'s ordering precedent.
//! One wiremock server backs both surfaces, so ports are identical and
//! need no normalization.
//!
//! Run with:
//!   cargo nextest run -p webfang_mcp --features mcp --test cli_mcp_export_jsonl_parity_test
//! (the `webfang` binary must exist in the active target dir; `cargo build`
//! or `cargo nextest`'s build step provides it.)

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use std::net::SocketAddr;
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wreq::Client;

use webfang_core::di::Container;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::CrawlerConfig;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::server::ServerOptions;
use webfang_mcp::mcp_server::state::McpState;

// ============================================================================
// JSON-RPC harness — local copies (each integration test binary is standalone)
// ============================================================================

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
            "clientInfo": { "name": "cli-mcp-parity-test", "version": "1.0.0" }
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
    // `result` here is the JSON-RPC result object (already unwrapped by
    // `tool_result`), whose content array carries the text part.
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

/// Unwrap the JSON-RPC envelope: the tool payload lives under `result`
/// (same convention as the shared `export_coverage`/`mcp_behavioral` harness).
fn tool_result(resp: Value) -> Value {
    resp.get("result")
        .cloned()
        .unwrap_or_else(|| panic!("expected a JSON-RPC result, got: {resp}"))
}

/// A relative temporary directory that deletes itself on drop.
///
/// `tempfile::TempDir` always returns an absolute path (it joins with
/// `env::current_dir`), which the MCP `require_safe_path` validator rejects.
/// These tests need a *relative* output dir, so we manage one manually.
struct RelTempDir {
    path: std::path::PathBuf,
}

impl RelTempDir {
    fn new(prefix: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let name = format!("{prefix}-{}-{}", std::process::id(), n);
        let path = std::path::PathBuf::from(name);
        std::fs::create_dir_all(&path).expect("create relative temp dir");
        RelTempDir { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for RelTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ============================================================================
// Fixtures & normalization
// ============================================================================

/// Start an in-process MCP server with the SSRF guards disabled — the exact
/// environment the proven multi-page crawl test
/// (`test_crawl_site_max_depth_one_follows_internal_links`,
/// `ssrf_guards_off`) runs under: the MCP-layer gate AND the entry-layer
/// gate, because wiremock binds 127.0.0.1. Setup is serialized under
/// ENV_LOCK via `Once` + `env_set` (#1126), so it stays safe even if
/// this binary gains more tests.
async fn start_server() -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    // Process-wide, permanent setup (no restore-on-drop): `env_set`
    // acquires ENV_LOCK itself, so each mutation stays serialized under
    // the workspace ENV_LOCK invariant (issue #1126) without a manual
    // `env_lock` binding.
    // Both SSRF layers are lifted: the MCP entry validator and the
    // shared core literal-IP entry guard (F-06 + F-32, #1217), since
    // every tool under test fetches wiremock loopback literals.
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        webfang_test_utils::env_set(
            webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV,
            "1",
        );
        webfang_test_utils::env_set(
            webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        );
    });

    let container_tmp = tempfile::TempDir::new().expect("create container temp dir");
    let crawler_config =
        CrawlerConfig::new(url::Url::parse("https://seed.invalid").expect("valid seed URL"));
    let scraper_config = ScraperConfig {
        output_dir: container_tmp.path().to_path_buf(),
        ..Default::default()
    };
    let container = Container::new(crawler_config, scraper_config)
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
    (base_url, handle, container_tmp)
}

/// Resolve the `webfang` CLI binary. In this virtual workspace the binary is
/// built by the sibling `webfang_cli` crate, so `CARGO_BIN_EXE_*` is not set
/// for this test binary; the active `CARGO_TARGET_DIR` (or the workspace
/// `target/`) is the honest lookup path once the suite has been built.
fn webfang_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return std::path::PathBuf::from(p);
    }
    let name = if cfg!(windows) {
        "webfang.exe"
    } else {
        "webfang"
    };
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("resolve workspace root");
    let candidates = [
        std::env::var("CARGO_TARGET_DIR")
            .ok()
            .map(std::path::PathBuf::from),
        Some(workspace_root.join("target")),
        Some(manifest_dir.join("target")),
    ];
    for candidate in candidates.into_iter().flatten() {
        let path = candidate.join("debug").join(name);
        if path.exists() {
            return path;
        }
    }
    panic!("webfang binary not found — build it first (cargo build -p webfang_cli)");
}

/// One wiremock page: a title, two content paragraphs (comfortably above the
/// minimum-content guard, so the pipeline never drops a page for thin text),
/// and optional raw link markup appended for the seed. Plain string concat —
/// no escaping surprises. Each page carries a unique marker so a mix-up
/// between surfaces is impossible.
fn page(marker: &str, links: &str) -> String {
    let body = format!(
        "<h1>Heading {marker}</h1><p>Marker {marker}: the quick brown fox jumps over the lazy dog \
         while the parity fixture holds its ground and repeats enough ordinary sentences \
         for the readability pipeline to accept this document as the main content of the \
         page without tripping the minimum content guard at any surface.</p>\
         <p>A second paragraph for {marker} keeps the extracted text comfortably above the guard \
         with more plain words that carry no links, no scripts, and no noise at all here.</p>"
    );
    format!("<html><head><title>Parity {marker}</title></head><body>{links}{body}</body></html>")
}

/// Read a JSONL export into timestamp-redacted records sorted by url.
///
/// `timestamp_utc` is the only per-run field of the shared record shape;
/// everything else (checksum, word count, metadata version, content) must
/// match across surfaces byte-for-byte.
fn normalized_records(path: &std::path::Path) -> Vec<Value> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let mut records: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut v: Value = serde_json::from_str(l).expect("export line must be JSON");
            if let Some(obj) = v.as_object_mut() {
                if obj.contains_key("timestamp_utc") {
                    obj.insert(
                        "timestamp_utc".to_string(),
                        Value::String("[TIMESTAMP]".to_string()),
                    );
                }
            }
            v
        })
        .collect();
    records.sort_by(|a, b| {
        a.get("url")
            .or_else(|| a.get("failed_url"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(
                b.get("url")
                    .or_else(|| b.get("failed_url"))
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
    });
    records
}

// ============================================================================
// The parity test (closes P6-2/F-16)
// ============================================================================

/// Mount the shared fixture site: permissive robots.txt, a seed with two
/// internal links, and the two leaf pages.
async fn mount_parity_site(site: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /\n"))
        .mount(site)
        .await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(page("seed", r#"<a href="/a">a</a> <a href="/b">b</a>"#)),
        )
        .mount(site)
        .await;
    Mock::given(method("GET"))
        .and(path("/a"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page("alpha", "")))
        .mount(site)
        .await;
    Mock::given(method("GET"))
        .and(path("/b"))
        .respond_with(ResponseTemplate::new(200).set_body_string(page("beta", "")))
        .mount(site)
        .await;
}

/// MCP surface: `crawl_site` + `export_jsonl` over the session buffer,
/// returning the redacted records. The server and its output dir live for
/// the whole call; records are read before they drop.
async fn mcp_export_records(seed: &str) -> Vec<Value> {
    let (mcp_base, _mcp_server, _mcp_tmp) = start_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &mcp_base).await;

    let crawl = tool_result(
        call_tool(
            &client,
            &mcp_base,
            &session_id,
            "crawl_site",
            json!({ "url": seed, "max_depth": 2, "max_pages": 10 }),
        )
        .await,
    );
    let crawl_response = tool_text(&crawl);
    assert!(
        !is_tool_error(&crawl),
        "crawl_site must succeed: {crawl_response}"
    );
    let crawled: Value =
        serde_json::from_str(&crawl_response).expect("crawl_site response is JSON");
    // A crawl that visited anything else would be an engine/fixture failure,
    // not an export failure: fail HERE, loudly, not in the parity assert.
    assert_eq!(
        crawled.get("total_pages").and_then(Value::as_u64),
        Some(3),
        "the MCP crawl must visit seed + two leaf pages: {crawl_response}"
    );

    let mcp_out = RelTempDir::new("cli-mcp-parity-mcp");
    let export = tool_result(
        call_tool(
            &client,
            &mcp_base,
            &session_id,
            "export_jsonl",
            json!({
                "output_dir": mcp_out.path().to_string_lossy(),
                "filename": "parity",
            }),
        )
        .await,
    );
    assert!(
        !is_tool_error(&export),
        "export_jsonl must succeed on the session buffer: {}",
        tool_text(&export)
    );
    normalized_records(&mcp_out.path().join("parity.jsonl"))
}

/// CLI surface: the `webfang` binary runs the same crawl knobs with the
/// JSONL pipeline export, returning the redacted records from the single
/// `*.jsonl` it writes.
fn cli_export_records(seed: &str) -> Vec<Value> {
    let cli_tmp = tempfile::TempDir::new().expect("cli temp dir");
    let cli_out = cli_tmp.path().join("export");
    let cache = cli_tmp.path().join("cache");
    std::fs::create_dir_all(&cache).expect("create hermetic cache dir");

    let mut cmd = std::process::Command::new(webfang_binary());
    // Same hygiene as the CLI behavioral harness: a poisoned env (CI
    // workflows export WEBFANG_*) must not leak into the run, and ONLY the
    // entry-layer SSRF guard is disarmed for the 127.0.0.1 wiremock literal.
    for key in std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("WEBFANG_"))
    {
        cmd.env_remove(&key);
    }
    let cli_output = cmd
        .arg(seed)
        .args(["--max-depth", "2", "--max-pages", "10"])
        .args(["--output", cli_out.to_string_lossy().as_ref()])
        .args(["--pipeline-format", "jsonl"])
        .arg("--quiet")
        .env(
            webfang_core::domain::ssrf_guard::DISABLE_ENTRY_GUARD_ENV,
            "1",
        )
        .env("XDG_CACHE_HOME", &cache)
        .output()
        .expect("spawn the webfang CLI");
    assert!(
        cli_output.status.success(),
        "CLI crawl+export must succeed — stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&cli_output.stdout),
        String::from_utf8_lossy(&cli_output.stderr),
    );

    let jsonls: Vec<_> = walkdir::walk_dir(&cli_out)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    assert_eq!(
        jsonls.len(),
        1,
        "CLI must write exactly one JSONL export, found: {jsonls:?}"
    );
    normalized_records(&jsonls[0])
}

/// One wiremock site, two surfaces, one record set: the CLI crawl's JSONL
/// export and the MCP `crawl_site` + `export_jsonl` sequence must produce
/// record-equivalent JSONL modulo the redacted run timestamp (P6-2/F-16,
/// #1290). Explicit depth/page budgets keep the two runs on identical
/// engine knobs despite the per-surface advertised defaults (#940).
#[tokio::test]
async fn cli_vs_mcp_export_jsonl_record_parity() {
    let site = MockServer::start().await;
    mount_parity_site(&site).await;
    let seed = format!("{}/", site.uri());

    let mcp_records = mcp_export_records(&seed).await;
    let cli_records = cli_export_records(&seed);

    // --- record equivalence -------------------------------------------------
    assert!(!cli_records.is_empty(), "the CLI export must hold records");
    assert_eq!(
        cli_records, mcp_records,
        "P6-2/F-16: CLI and MCP JSONL exports over the same site must be record-equivalent modulo timestamp_utc\nCLI: {cli_records:#?}\nMCP: {mcp_records:#?}"
    );

    // The equivalence above is the proof; these pins keep a degenerate
    // (both-sides-wrong) pass impossible: the shared shape is asserted too.
    assert_eq!(cli_records.len(), 3, "seed + two leaf pages");
    for record in &cli_records {
        assert_eq!(
            record.get("metadata_version").and_then(Value::as_str),
            Some("2.1.0"),
            "records carry the RC-1 shared shape: {record}"
        );
        let checksum = record
            .get("checksum_sha256")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            checksum.len(),
            64,
            "checksum is a full SHA-256 hex digest: {record}"
        );
        assert!(
            record
                .get("word_count")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                > 20,
            "extracted content is the readable body, not boilerplate: {record}"
        );
        assert_eq!(
            record.get("timestamp_utc").and_then(Value::as_str),
            Some("[TIMESTAMP]"),
            "the redaction itself proves the field is present per record"
        );
    }
    let urls: Vec<&str> = cli_records
        .iter()
        .filter_map(|r| r.get("url").and_then(Value::as_str))
        .collect();
    assert!(
        urls.contains(&seed.as_str())
            && urls.iter().any(|u| u.ends_with("/a"))
            && urls.iter().any(|u| u.ends_with("/b")),
        "every fixture page reached both exports: {urls:?}"
    );
}

mod walkdir {
    use std::path::{Path, PathBuf};

    /// Minimal recursive file walk (the workspace ships no `walkdir` dep).
    pub fn walk_dir(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    files.push(path);
                }
            }
        }
        files
    }
}
