//! The MCP export result carries the count of lines skipped for not being
//! UTF-8 (issue #2238).
//!
//! The count exists in the export phase — #1812 decided the policy: a line
//! that is not valid UTF-8 is SKIPPED, the export COMPLETES, and the damage
//! is reported through a structured ERROR (`stage = "jsonl_resume_index"`,
//! `ErrorKind::InvalidData`). But that count never travelled to the MCP
//! surface: a client that exported a damaged JSONL had no way of knowing that
//! lines were dropped, because the ERROR lives in a log the client does not
//! read (and the CLI's console level is `warn` by default, so even the
//! operator needs `-v` or `--trace-file`).
//!
//! The field is ADDITIVE, and this suite is where that promise is pinned:
//!
//! - the count travels as `structuredContent.skipped_non_utf8` (a JSON
//!   number) — a NEW key, so a client that reads the result's text keeps
//!   working byte for byte;
//! - the text itself gains ONE line, and only when there is damage to report;
//! - a clean export's text is EXACTLY what it was before the field existed;
//! - the tool registry is untouched: adding a field to a RESULT does not add
//!   a tool (the 36-tool pin lives in `stdio_handshake.rs`).
//!
//! Run with:
//! cargo nextest run -p webfang_mcp --features mcp --test mcp_export_skipped_count_test

#![cfg(feature = "mcp")]

use serde_json::json;
use wreq::Client;

mod common;
use common::{call_tool, init_session, is_tool_error, start_session_seeded_server, tool_text};

/// The export file name the tests write.
const EXPORT_NAME: &str = "export";

/// A JSONL export file whose SECOND line is not valid UTF-8.
///
/// The first line is a well-formed record — the file must look like an export
/// a previous run left behind — and the second one carries bytes that cannot
/// be decoded: exactly the damage the export phase skips (#1812). The file
/// ends with a newline, so the writer's torn-tail recovery leaves it alone
/// and the damaged line survives to be counted.
const DAMAGED_EXPORT: &[u8] =
    b"{\"content\":\"ok\",\"checksum_sha256\":\"a\"}\n{\"content\":\"\xff\xfe\",\"checksum_sha256\":\"b\"}\n";

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

/// The count of skipped lines the tool result carries, read from its
/// structured content.
fn skipped_count(result: &serde_json::Value) -> Option<u64> {
    result
        .get("structuredContent")
        .and_then(|sc| sc.get("skipped_non_utf8"))
        .and_then(serde_json::Value::as_u64)
}

/// #2238: an export over a JSONL file with a non-UTF-8 line reports the count
/// of the skipped lines — the count the ERROR in the log already carried, now
/// on the surface the client actually reads.
#[tokio::test]
async fn export_jsonl_reports_the_skipped_non_utf8_lines() {
    let (base_url, _handle, _container_tmp) = start_session_seeded_server(1).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out-skipped");
    std::fs::write(
        out.path().join(format!("{EXPORT_NAME}.jsonl")),
        DAMAGED_EXPORT,
    )
    .expect("seed the damaged export file");

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_jsonl",
        json!({ "output_dir": out.path().to_string_lossy(), "filename": EXPORT_NAME }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "the export must succeed: {}",
        tool_text(&result)
    );

    // The machine-readable form: a NEW structured key, `skipped_non_utf8`,
    // carrying a number.
    assert_eq!(
        skipped_count(&result),
        Some(1),
        "the result must carry the count of the skipped non-UTF-8 lines: {result}"
    );

    // The human-readable form: the text names the count, so a client that
    // only surfaces the text to a model still learns that lines were dropped.
    let text = tool_text(&result);
    assert!(
        text.contains("skipped_non_utf8"),
        "the text must name the count: {text}"
    );
    assert!(
        text.contains("Exportación completada"),
        "the existing text must survive: {text}"
    );
}

/// The additive promise has a second half: a CLEAN export must not move.
/// The count is 0 and the text carries no new line, so a client that already
/// reads the result sees exactly what it saw before the field existed.
#[tokio::test]
async fn export_jsonl_without_damage_reports_zero_and_keeps_the_text() {
    let (base_url, _handle, _container_tmp) = start_session_seeded_server(1).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out-clean");
    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_jsonl",
        json!({ "output_dir": out.path().to_string_lossy(), "filename": EXPORT_NAME }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "the export must succeed: {}",
        tool_text(&result)
    );
    assert_eq!(
        skipped_count(&result),
        Some(0),
        "a clean export must report zero skipped lines: {result}"
    );

    let text = tool_text(&result);
    assert!(
        !text.contains("skipped_non_utf8"),
        "a clean export must not mention the count: {text}"
    );
    assert!(
        text.contains("Exportación completada") && text.contains("archivo de procedencia"),
        "the clean text must stay exactly what it was: {text}"
    );
}

/// #2238: the count is a property of the export FAMILY, not of one tool:
/// `export_file` reports it too.
#[tokio::test]
async fn export_file_reports_the_skipped_non_utf8_lines() {
    let (base_url, _handle, _container_tmp) = start_session_seeded_server(1).await;
    let client = Client::new();
    let session_id = init_session(&client, &base_url).await;

    let out = RelTempDir::new("wf-out-file");
    std::fs::write(
        out.path().join(format!("{EXPORT_NAME}.jsonl")),
        DAMAGED_EXPORT,
    )
    .expect("seed the damaged export file");

    let resp = call_tool(
        &client,
        &base_url,
        &session_id,
        "export_file",
        json!({
            "output_dir": out.path().to_string_lossy(),
            "filename": EXPORT_NAME,
            "content_format": "jsonl",
            "content": "hello"
        }),
    )
    .await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("expected result, got: {resp}"))
        .clone();
    assert!(
        !is_tool_error(&result),
        "the export must succeed: {}",
        tool_text(&result)
    );
    assert_eq!(
        skipped_count(&result),
        Some(1),
        "the result must carry the count of the skipped non-UTF-8 lines: {result}"
    );
}

/// #2238 (S3): adding a field to a RESULT does not add a tool. The registry
/// the client sees is still the 36-tool one that `stdio_handshake.rs` pins
/// over the wire — this assertion is the in-process view of that same pin.
#[test]
fn the_tool_registry_is_unchanged_by_the_new_result_field() {
    let router = webfang_mcp::mcp_server::handlers::build_tool_router();
    assert_eq!(
        router.map.len(),
        36,
        "the registry must still expose 36 tools: adding a field to a result \
         does not add a tool"
    );
}
