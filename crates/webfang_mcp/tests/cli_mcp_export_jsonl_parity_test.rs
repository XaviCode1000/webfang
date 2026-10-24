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
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wreq::Client;

// Canonical HTML page fixture, shared across the MCP test binaries (#1371).
// Imported by name, not `use common::*`: this file keeps its own
// crawl-specific fixtures below. The JSON-RPC client (`call_tool`,
// `init_session_with_name`, `tool_text`, `is_tool_error`), the server
// bootstrap (`start_server`) and the arrange helpers (`RelTempDir`,
// `webfang_binary`, `normalized_records`, `walk_dir`) are shared via
// `common` (#1757, S2).
mod common;
use common::{
    call_tool, init_session_with_name, is_tool_error, mount_page_200, normalized_records,
    start_server, tool_text, walk_dir, webfang_binary, RelTempDir,
};

/// Unwrap the JSON-RPC envelope: the tool payload lives under `result`
/// (same convention as the shared `export_coverage`/`mcp_behavioral` harness).
fn tool_result(resp: Value) -> Value {
    resp.get("result")
        .cloned()
        .unwrap_or_else(|| panic!("expected a JSON-RPC result, got: {resp}"))
}

// ============================================================================
// Fixtures & normalization
// ============================================================================

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

// ============================================================================
// The parity test (closes P6-2/F-16)
// ============================================================================

/// Mount the shared fixture site: permissive robots.txt, a seed with two
/// internal links, and the two leaf pages. The pages go through
/// [`mount_page_200`] so an HTML fixture cannot drift back to wiremock's
/// `text/plain` default (#1354, #1371); robots.txt keeps its own mount,
/// where text/plain is the honest MIME. Both surfaces of the parity read
/// the same server, so the wire change applies to CLI and MCP alike.
async fn mount_parity_site(site: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /\n"))
        .mount(site)
        .await;
    mount_page_200(
        site,
        "/",
        &page("seed", r#"<a href="/a">a</a> <a href="/b">b</a>"#),
    )
    .await;
    mount_page_200(site, "/a", &page("alpha", "")).await;
    mount_page_200(site, "/b", &page("beta", "")).await;
}

/// MCP surface: `crawl_site` + `export_jsonl` over the session buffer,
/// returning the redacted records. The server and its output dir live for
/// the whole call; records are read before they drop.
async fn mcp_export_records(seed: &str) -> Vec<Value> {
    let (mcp_base, _mcp_server) = start_server(None).await;
    let client = Client::new();
    let session_id = init_session_with_name(&client, &mcp_base, "cli-mcp-parity-test").await;

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

    let jsonls: Vec<_> = walk_dir(&cli_out)
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
