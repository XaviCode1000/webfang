//! Staleness-differential E2E + CLI-vs-MCP sitemap parity (issue #1429, Phase 4).
//!
//! 4.1 (success criterion 1): two wiremock sites, one MCP session —
//! `crawl_site(A)` → `export_jsonl` (baseline = run A) →
//! `crawl_with_sitemap(B)` → `export_jsonl` — the second export MUST serve
//! run-B records with zero run-A records. Pre-Phase-2 the second export
//! silently re-served run A (the handler-level RED pin
//! `crawl_with_sitemap_stores_sitemap_run_in_session_results` captured the
//! same contract one layer down); this file pins it at the MCP protocol.
//!
//! 4.3 (success criterion 3): the `crawl_with_sitemap` response is parsed as
//! `Vec<String>` at the protocol level — the bare-array wire shape pin.
//!
//! 4.4 (success criterion 2, D5.3): same sitemap fixture via the CLI sitemap
//! path (`--use-sitemap --sitemap-url`) and via the MCP session entry;
//! `WebfangMetadata` JSONL record/field equality modulo transport on the
//! shared URL set. BFS-vs-sequential URL-set divergence is documented, not
//! asserted equal.
//!
//! Run with:
//!   cargo nextest run -p webfang_mcp --features mcp --test sitemap_crawl_run_staleness_test

#![cfg(feature = "mcp")]

use serde_json::{json, Value};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wreq::Client;

use webfang_core::di::Container;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::CrawlerConfig;
use webfang_mcp::mcp_server::server::{build_mcp_router, ServerOptions};
use webfang_mcp::mcp_server::state::McpState;

// This binary keeps its own server/session harness (a glob import would clash
// with the session/call/text/error helper names), so only the non-colliding
// page-fixture and port-serving helpers are imported by name.
mod common;
use common::{
    call_tool, init_session, is_tool_error, mount_page_200, serve_on_random_port, tool_text,
};

// ============================================================================
// Harness — local copies (each integration test binary is standalone)
// ============================================================================

/// Start an in-process MCP server with both SSRF layers lifted — the exact
/// environment the proven multi-page crawl test runs under (F-06 + F-32,
/// #1217), since every tool under test fetches wiremock loopback literals.
/// Setup is serialized under ENV_LOCK via `Once` + `env_set` (#1126).
/// Serving reuses the shared port helper so the bind/serve/wait sequence
/// lives in exactly one place (#1371).
async fn start_server() -> (String, tokio::task::JoinHandle<()>, tempfile::TempDir) {
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

    let (base_url, handle) = serve_on_random_port(app).await;
    (base_url, handle, container_tmp)
}

/// Unwrap the JSON-RPC envelope: the tool payload lives under `result`.
fn tool_result(resp: Value) -> Value {
    resp.get("result")
        .cloned()
        .unwrap_or_else(|| panic!("expected a JSON-RPC result, got: {resp}"))
}

/// A relative temporary directory that deletes itself on drop.
///
/// `tempfile::TempDir` always returns an absolute path, which the MCP
/// `require_safe_path` validator rejects. These tests need a *relative*
/// output dir, so we manage one manually — named after the process plus
/// a nanos timestamp, which keeps names unique without a global counter.
struct RelTempDir {
    path: std::path::PathBuf,
}

impl RelTempDir {
    fn new(prefix: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::path::PathBuf::from(format!("{prefix}-{}-{nanos}", std::process::id()));
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
// Fixtures
// ============================================================================

/// One wiremock page: a title plus two content paragraphs (comfortably above
/// the minimum-content guard, so the pipeline never drops a page for thin
/// text), with optional raw link markup. Each page carries a unique marker so
/// a mix-up between runs is impossible.
fn page(marker: &str, links: &str) -> String {
    let body = format!(
        "<h1>Heading {marker}</h1><p>Marker {marker}: the quick brown fox jumps over the lazy dog \
         while the staleness fixture holds its ground and repeats enough ordinary sentences \
         for the readability pipeline to accept this document as the main content of the \
         page without tripping the minimum content guard at any surface.</p>\
         <p>A second paragraph for {marker} keeps the extracted text comfortably above the guard \
         with more plain words that carry no links, no scripts, and no noise at all here.</p>"
    );
    format!("<html><head><title>Staleness {marker}</title></head><body>{links}{body}</body></html>")
}

/// Permissive robots.txt so the crawl never stalls on robots checks.
async fn mount_open_robots(site: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /\n"))
        .mount(site)
        .await;
}

/// Serve `/sitemap.xml` listing `leaves` — shared by the staleness site and
/// the parity site so the XML envelope lives in exactly one place.
async fn mount_sitemap_xml(site: &MockServer, leaves: &[&str]) {
    let base = site.uri();
    let entries = leaves
        .iter()
        .map(|leaf| format!("<url><loc>{base}{leaf}</loc></url>"))
        .collect::<Vec<_>>()
        .join("\n");
    let sitemap = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n{entries}\n</urlset>"
    );
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/xml")
                .set_body_string(sitemap),
        )
        .mount(site)
        .await;
}

/// Site A: a BFS crawl fixture — seed links two leaves.
async fn mount_site_a(site: &MockServer) {
    mount_open_robots(site).await;
    mount_page_200(
        site,
        "/",
        &page("seed-a", r#"<a href="/a1">a1</a> <a href="/a2">a2</a>"#),
    )
    .await;
    mount_page_200(site, "/a1", &page("alpha-one", "")).await;
    mount_page_200(site, "/a2", &page("alpha-two", "")).await;
}

/// Site B: a sitemap fixture — the seed carries NO links (sitemap seeds drive
/// coverage) and is ABSENT from the sitemap, pinning the spec scenario "seed
/// included when absent from sitemap".
async fn mount_site_b(site: &MockServer) {
    mount_open_robots(site).await;
    mount_page_200(site, "/", &page("seed-b", "")).await;
    mount_page_200(site, "/b1", &page("beta-one", "")).await;
    mount_page_200(site, "/b2", &page("beta-two", "")).await;
    mount_sitemap_xml(site, &["/b1", "/b2"]).await;
}

// ============================================================================
// Record helpers
// ============================================================================

/// Read a JSONL export into raw records.
fn read_records(path: &std::path::Path) -> Vec<Value> {
    let text =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("export line must be JSON"))
        .collect()
}

/// The `url` field of every record.
fn record_urls(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .filter_map(|r| r.get("url").and_then(Value::as_str))
        .collect()
}

/// The `url` field of a record, for ordering — shared by the snapshot
/// rendering and the timestamp-normalized comparison so the sort key lives
/// in exactly one place.
fn url_key(record: &Value) -> &str {
    record
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Deterministic rendering of an export for snapshots: records sorted by url,
/// one compact JSON object per line.
fn sorted_render(records: &[Value]) -> String {
    let mut sorted: Vec<&Value> = records.iter().collect();
    sorted.sort_by(|a, b| url_key(a).cmp(url_key(b)));
    sorted
        .iter()
        .map(|v| serde_json::to_string(v).expect("record must serialize"))
        .collect::<Vec<_>>()
        .join("\n")
}

// ============================================================================
// 4.1 — Staleness-differential E2E (success criterion 1)
// ============================================================================

/// Call `export_jsonl` for `filename` into `out`, assert success, and read
/// the records back. Shared by the baseline and sitemap-run exports below
/// — and factored out so the staleness E2E stays under the workspace
/// `too_many_lines` ratchet (#516).
async fn export_records(
    client: &Client,
    mcp_base: &str,
    session_id: &str,
    out: &RelTempDir,
    filename: &str,
    success_msg: &str,
) -> Vec<Value> {
    let export = tool_result(
        call_tool(
            client,
            mcp_base,
            session_id,
            "export_jsonl",
            json!({ "output_dir": out.path().to_string_lossy(), "filename": filename }),
        )
        .await,
    );
    assert!(
        !is_tool_error(&export),
        "{success_msg}: {}",
        tool_text(&export)
    );
    read_records(&out.path().join(format!("{filename}.jsonl")))
}

/// GREEN (strict TDD): the fixed-tree contract — the second export serves
/// run B (seed + sitemap pages, seed included although absent from the
/// sitemap) with zero run-A records. The RED version of this test asserted
/// the pre-fix stale expectation and failed with run-B URLs served.
#[tokio::test]
async fn export_after_sitemap_serves_sitemap_run_not_stale() {
    let site_a = MockServer::start().await;
    mount_site_a(&site_a).await;
    let site_b = MockServer::start().await;
    mount_site_b(&site_b).await;
    let seed_a = format!("{}/", site_a.uri());
    let seed_b = format!("{}/", site_b.uri());

    let (mcp_base, _server, _tmp) = start_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &mcp_base).await;

    // Baseline: crawl_site(A) + export serves run A.
    let crawl_a = tool_result(
        call_tool(
            &client,
            &mcp_base,
            &session_id,
            "crawl_site",
            json!({ "url": seed_a, "max_depth": 2, "max_pages": 10 }),
        )
        .await,
    );
    assert!(
        !is_tool_error(&crawl_a),
        "crawl_site(A) must succeed: {}",
        tool_text(&crawl_a)
    );
    let out = RelTempDir::new("sitemap-staleness");
    let records_a = export_records(
        &client,
        &mcp_base,
        &session_id,
        &out,
        "run-a",
        "baseline export must succeed",
    )
    .await;
    assert_eq!(records_a.len(), 3, "run A covers seed + two leaves");
    insta::assert_snapshot!(
        "staleness_baseline_run_a",
        webfang_test_utils::redact_nondeterministic(out.path(), &sorted_render(&records_a))
    );

    // Sitemap run: crawl_with_sitemap(B) — bounds omitted to exercise the
    // Phase-3 CRAWL_SITE defaults end to end.
    let crawl_b = tool_result(
        call_tool(
            &client,
            &mcp_base,
            &session_id,
            "crawl_with_sitemap",
            json!({ "url": seed_b, "sitemap_url": format!("{}/sitemap.xml", site_b.uri()) }),
        )
        .await,
    );
    assert!(
        !is_tool_error(&crawl_b),
        "crawl_with_sitemap(B) must succeed: {}",
        tool_text(&crawl_b)
    );
    // 4.3 wire pin at the protocol level: the response stays a bare JSON
    // string array of discovered sitemap URLs.
    let discovered: Vec<String> =
        serde_json::from_str(&tool_text(&crawl_b)).expect("response must stay a bare JSON array");
    assert!(
        discovered.iter().any(|u| u.ends_with("/b1"))
            && discovered.iter().any(|u| u.ends_with("/b2")),
        "response array must carry the sitemap pages, got: {discovered:?}"
    );

    // Second export serves the sitemap run (success criterion 1): run-B
    // records present, zero run-A records. The RED version of this test
    // asserted the pre-fix stale expectation (`any seed_a`) and failed with
    // run-B URLs served — the E2E discriminates the contract it pins.
    let records_b = export_records(
        &client,
        &mcp_base,
        &session_id,
        &out,
        "run-b",
        "second export must succeed",
    )
    .await;
    let urls_b = record_urls(&records_b);
    assert!(
        !urls_b.iter().any(|u| u.starts_with(&seed_a)),
        "second export must hold zero run-A records, got: {urls_b:?}"
    );
    assert!(
        urls_b.contains(&seed_b.as_str()),
        "seed B crawled although absent from the sitemap, got: {urls_b:?}"
    );
    assert!(
        urls_b.iter().any(|u| u.ends_with("/b1")) && urls_b.iter().any(|u| u.ends_with("/b2")),
        "sitemap pages present in the stored run, got: {urls_b:?}"
    );
    insta::assert_snapshot!(
        "staleness_sitemap_run_b",
        webfang_test_utils::redact_nondeterministic(out.path(), &sorted_render(&records_b))
    );
}

// ============================================================================
// 4.4 — CLI-vs-MCP sitemap parity (success criterion 2, D5.3)
// ============================================================================

/// Resolve the `webfang` CLI binary — the same honest lookup as
/// `cli_mcp_export_jsonl_parity_test`: `CARGO_BIN_EXE_webfang` is unset for
/// this test binary (the binary is built by sibling `webfang_cli`), so the
/// active `CARGO_TARGET_DIR` (the isolated per-worktree dir in this repo)
/// is probed first, then the workspace `target/`.
fn webfang_binary() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_webfang") {
        return std::path::PathBuf::from(p);
    }
    let name = if cfg!(windows) {
        "webfang.exe"
    } else {
        "webfang"
    };
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dirs = [
        std::env::var("CARGO_TARGET_DIR")
            .ok()
            .map(std::path::PathBuf::from),
        manifest
            .parent()
            .and_then(|p| p.parent())
            .map(|root| root.join("target")),
        Some(manifest.join("target")),
    ];
    target_dirs
        .into_iter()
        .flatten()
        .map(|dir| dir.join("debug").join(name))
        .find(|candidate| candidate.exists())
        .expect("webfang binary not found — build it first (cargo build -p webfang_cli)")
}

/// Minimal recursive file walk (the workspace ships no `walkdir` dep).
mod walkdir {
    use std::path::{Path, PathBuf};

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

/// Same sitemap shape as site B above, relabelled for the parity run: a
/// link-free seed ABSENT from the sitemap plus two sitemap-only leaves.
async fn mount_parity_sitemap_site(site: &MockServer) {
    mount_open_robots(site).await;
    mount_page_200(site, "/", &page("parity-seed", "")).await;
    mount_page_200(site, "/c1", &page("parity-one", "")).await;
    mount_page_200(site, "/c2", &page("parity-two", "")).await;
    mount_sitemap_xml(site, &["/c1", "/c2"]).await;
}

/// Read a JSONL export into timestamp-normalized records sorted by url.
///
/// `timestamp_utc` is the one per-run field of the shared `WebfangMetadata`
/// shape; everything else (url, title, content, checksum, word count, …)
/// must match across surfaces byte-for-byte on the shared URL set.
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
    records.sort_by(|a, b| url_key(a).cmp(url_key(b)));
    records
}

/// Key records by their `url` field for shared-set comparison.
fn keyed_by_url(records: &[Value]) -> std::collections::HashMap<&str, &Value> {
    records
        .iter()
        .filter_map(|r| r.get("url").and_then(Value::as_str).map(|u| (u, r)))
        .collect()
}

/// MCP surface: the new session entry (`crawl_with_sitemap`) followed by
/// the session export, returning the normalized records. The server and
/// its output dir live for the whole call; records are read before they
/// drop. `start_server` runs first: its process-wide `Once` writes the
/// environment via `env_set` (ENV_LOCK), which must fire BEFORE any
/// `EnvGuard` scope exists — no guard is held anywhere on this path, so
/// the #1224 nesting invariant holds trivially (#1126).
async fn mcp_sitemap_run_records(seed: &str, sitemap_url: &str) -> Vec<Value> {
    let (mcp_base, _server, _tmp) = start_server().await;
    let client = Client::new();
    let session_id = init_session(&client, &mcp_base).await;

    let crawl = tool_result(
        call_tool(
            &client,
            &mcp_base,
            &session_id,
            "crawl_with_sitemap",
            json!({ "url": seed, "sitemap_url": sitemap_url, "max_depth": 2, "max_pages": 10 }),
        )
        .await,
    );
    assert!(
        !is_tool_error(&crawl),
        "crawl_with_sitemap must succeed: {}",
        tool_text(&crawl)
    );

    let mcp_out = RelTempDir::new("sitemap-parity-mcp");
    let export = tool_result(
        call_tool(
            &client,
            &mcp_base,
            &session_id,
            "export_jsonl",
            json!({ "output_dir": mcp_out.path().to_string_lossy(), "filename": "parity" }),
        )
        .await,
    );
    assert!(
        !is_tool_error(&export),
        "MCP export must succeed: {}",
        tool_text(&export)
    );
    normalized_records(&mcp_out.path().join("parity.jsonl"))
}

/// CLI surface: the `webfang` binary over the same fixture
/// (`--use-sitemap --sitemap-url … --pipeline-format jsonl`), returning
/// the normalized records from the single `*.jsonl` it writes. Same
/// hygiene as the CLI behavioral harness and the reference parity test: a
/// poisoned env (CI workflows export `WEBFANG_*`) must not leak into the
/// run, and ONLY the entry-layer SSRF guard is disarmed for the
/// 127.0.0.1 wiremock literal — via the canonical const, never a literal
/// env name (#1382, #1126). The hatch is set on the CHILD `Command` env,
/// never the process env, so no `ENV_LOCK`/`EnvGuard` is involved.
fn cli_sitemap_run_records(seed: &str, sitemap_url: &str) -> Vec<Value> {
    let cli_tmp = tempfile::TempDir::new().expect("cli temp dir");
    let cli_out = cli_tmp.path().join("export");
    let cache = cli_tmp.path().join("cache");
    std::fs::create_dir_all(&cache).expect("create hermetic cache dir");

    let mut cmd = std::process::Command::new(webfang_binary());
    for key in std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with("WEBFANG_"))
    {
        cmd.env_remove(&key);
    }
    let cli_output = cmd
        .arg("--url")
        .arg(seed)
        .arg("--sitemap-url")
        .arg(sitemap_url)
        .arg("--use-sitemap")
        .args(["--max-depth", "2", "--max-pages", "10"])
        .arg("--output")
        .arg(&cli_out)
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
        "CLI sitemap crawl+export must succeed — stdout: {}\nstderr: {}",
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

/// 4.4 (success criterion 2, D5.3): the same sitemap fixture through the
/// CLI sitemap path and through the new MCP session entry.
///
/// Record/field equality is asserted ONLY on the shared URL set (the
/// sitemap leaves): the BFS-vs-sequential URL-set divergence is
/// DOCUMENTED, never asserted equal — MCP BFS-expands from the seed and
/// includes the seed page although absent from the sitemap, while CLI
/// `--sitemap` sequential mode treats the sitemap as the source of truth
/// and never scrapes the unlisted seed. Asserting the URL sets equal
/// would pin the divergence the spec accepts as a delta.
#[tokio::test]
async fn cli_vs_mcp_sitemap_run_record_shape_parity() {
    let site = MockServer::start().await;
    mount_parity_sitemap_site(&site).await;
    let seed = format!("{}/", site.uri());
    let sitemap_url = format!("{}/sitemap.xml", site.uri());

    let mcp_records = mcp_sitemap_run_records(&seed, &sitemap_url).await;
    let cli_records = cli_sitemap_run_records(&seed, &sitemap_url);

    // Snapshots pin each surface's full export. The records hold no temp
    // paths (only wiremock URLs), so the redact dir is a stable
    // non-occurring placeholder: temp-path redaction is a no-op while the
    // port regex still normalizes the ephemeral wiremock port. Passing the
    // real out-dirs is impossible here (they drop inside the helpers) and
    // passing "." is WRONG — it redacts every literal dot (127.0.0.1 →
    // 127<OUT_DIR>0…, observed on the first run).
    let no_temp_paths = std::path::Path::new("sitemap-parity-no-temp-paths");
    insta::assert_snapshot!(
        "sitemap_parity_mcp",
        webfang_test_utils::redact_nondeterministic(no_temp_paths, &sorted_render(&mcp_records))
    );
    insta::assert_snapshot!(
        "sitemap_parity_cli",
        webfang_test_utils::redact_nondeterministic(no_temp_paths, &sorted_render(&cli_records))
    );

    let mcp_urls: Vec<&str> = record_urls(&mcp_records);
    let cli_urls: Vec<&str> = record_urls(&cli_records);
    // MCP covers seed (absent from the sitemap) + sitemap leaves;
    // CLI covers the sitemap leaves only. Both halves of the delta are
    // pinned here so a future convergence/divergence is visible.
    assert!(
        mcp_urls.contains(&seed.as_str()),
        "MCP run includes the seed although absent from the sitemap, got: {mcp_urls:?}"
    );
    for leaf in ["/c1", "/c2"] {
        assert!(
            mcp_urls.iter().any(|u| u.ends_with(leaf)),
            "MCP run holds sitemap leaf {leaf}, got: {mcp_urls:?}"
        );
        assert!(
            cli_urls.iter().any(|u| u.ends_with(leaf)),
            "CLI run holds sitemap leaf {leaf}, got: {cli_urls:?}"
        );
    }

    // Shared-set record equality: every sitemap-leaf record identical
    // modulo the normalized run timestamp (D5.3).
    let mcp_by_url = keyed_by_url(&mcp_records);
    let cli_by_url = keyed_by_url(&cli_records);
    let shared: Vec<&&str> = mcp_by_url
        .keys()
        .filter(|u| cli_by_url.contains_key(**u))
        .collect();
    assert!(
        shared.len() == 2,
        "shared URL set is exactly the two sitemap leaves, got: {shared:?}"
    );
    for url in &shared {
        assert_eq!(
mcp_by_url[**url], cli_by_url[**url],
"D5.3: CLI and MCP records for shared sitemap URL {url} must match modulo timestamp_utc"
        );
    }

    // Shape pins keep a degenerate (both-sides-empty) pass impossible.
    for url in &shared {
        let record = cli_by_url[**url];
        assert_eq!(
            record.get("metadata_version").and_then(Value::as_str),
            Some("2.1.0"),
            "records carry the shared WebfangMetadata shape: {record}"
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
            "the normalization itself proves the field is present per record"
        );
    }
}
