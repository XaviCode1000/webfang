//! Stdio handshake + lifecycle integration test — regression guard for #759.
//!
//! Contract-based audit:
//! - Observable behavior only: the test drives the REAL binary through its
//!   public transport (JSON-RPC over stdin/stdout). No internal state.
//! - Ephemeral adapters: each test spawns its own short-lived child process
//!   (`kill_on_drop` prevents orphans on failure). No wiremock, no network,
//!   no host filesystem assumptions.
//! - Semantic assertions: the contract is the JSON-RPC shape — protocol
//!   version negotiation, serverInfo presence, tool registry size, tool call
//!   result — not raw byte dumps. No snapshots needed: value assertions ARE
//!   the semantic contract here.
//! - Absolute determinism: `extract_domain` is pure URL string logic, so the
//!   tests never depend on the network, hf_hub model resolution, or wall
//!   clock. Timeouts are generous (15 s per read) to absorb cold starts and
//!   BoringSSL initialization on constrained hardware.
//!
//! The regression guard (#759): with `--enable-ai`, the server used to block
//! the MCP `initialize` handshake behind hf_hub model resolution (~390 MB
//! download on a cold cache). Now the container boots fast, `serve()` starts
//! immediately, and the AI ports are wired lazily in a background task. These
//! tests assert the handshake is answered while that warmup is still pending.
//! The flag is passed on every spawn: it is honestly ignored on non-AI builds
//! and exercises the lazy wiring path on AI builds.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdout;

/// Per-read timeout: generous enough for cold starts + BoringSSL init.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// Timeout for the graceful-shutdown wait on stdin EOF.
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Spawn the real `webfang-mcp-stdio` binary with piped JSON-RPC stdio.
///
/// `kill_on_drop(true)` guarantees no orphan process survives a test panic.
fn spawn_stdio_server() -> tokio::process::Child {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_webfang-mcp-stdio"));
    cmd.arg("--enable-ai")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        // stderr carries tracing logs, never JSON-RPC. Null it so the child
        // can never block on an unread log buffer.
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    cmd.spawn().expect("spawn the webfang-mcp-stdio binary")
}

/// Same as [`spawn_stdio_server`] but with stderr piped, so #1108 failure-mode
/// tests can assert the absence of a panic backtrace on the child's stderr.
fn spawn_stdio_server_with_stderr() -> tokio::process::Child {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_webfang-mcp-stdio"));
    cmd.arg("--enable-ai")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    cmd.spawn().expect("spawn the webfang-mcp-stdio binary")
}

/// Drain the child's stderr to a string. Call after the child has exited.
async fn drain_stderr(child: &mut tokio::process::Child) -> String {
    let mut buf = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf)
            .await
            .expect("read the child's stderr to EOF");
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Wait for the child to exit, failing the test if it hangs.
async fn wait_exited(child: &mut tokio::process::Child) -> std::process::ExitStatus {
    tokio::time::timeout(EXIT_TIMEOUT, child.wait())
        .await
        .expect("server exited within the timeout")
        .expect("reap the server child process")
}

/// Write one JSON-RPC message (newline-delimited) to the server.
async fn send(stdin: &mut tokio::process::ChildStdin, message: &serde_json::Value) {
    let mut line = message.to_string();
    line.push('\n');
    stdin
        .write_all(line.as_bytes())
        .await
        .expect("write a JSON-RPC message to the server stdin");
    stdin.flush().await.expect("flush the server stdin buffer");
}

/// Read one JSON-RPC line from the server, failing the test after
/// [`READ_TIMEOUT`]. Under #759's regression this is where the test would
/// hang before the fix (the server never answered `initialize`).
async fn read_json_line(reader: &mut BufReader<ChildStdout>) -> serde_json::Value {
    let mut line = String::new();
    let bytes = tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .expect("server answered within the read timeout")
        .expect("read a JSON-RPC line from the server stdout");
    assert!(bytes > 0, "server closed stdout without answering");
    serde_json::from_str(&line).expect("each server message is one JSON object per line")
}

/// Drive the MCP handshake: `initialize` → `notifications/initialized`.
/// Returns the initialize response.
async fn handshake(
    stdin: &mut tokio::process::ChildStdin,
    reader: &mut BufReader<ChildStdout>,
) -> serde_json::Value {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "stdio-handshake-test", "version": "1"}
        }
    });
    send(stdin, &initialize).await;
    let response = read_json_line(reader).await;

    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    send(stdin, &initialized).await;

    response
}

/// Regression guard for #759: `initialize` is answered while AI warmup is
/// still pending. Pre-fix, the binary blocked this response behind hf_hub
/// model resolution and never answered within any reasonable time.
#[tokio::test]
async fn stdio_initialize_is_answered_before_ai_warmup_completes() {
    let mut child = spawn_stdio_server();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));

    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "stdio-handshake-test", "version": "1"}
        }
    });
    send(&mut stdin, &initialize).await;

    let response = read_json_line(&mut reader).await;
    assert_eq!(
        response["id"], 1,
        "the JSON-RPC response must echo the request id; got: {response}"
    );
    let result = &response["result"];
    // rmcp 1.8.0 negotiates exactly this protocol version (verified empirically).
    assert_eq!(
        result["protocolVersion"], "2025-03-26",
        "initialize must negotiate protocolVersion 2025-03-26; got: {response}"
    );
    assert!(
        result["serverInfo"].is_object(),
        "initialize result must carry serverInfo; got: {response}"
    );
}

/// `tools/list` reports the full 36-tool registry, including
/// `extract_domain` and `get_accessibility_snapshot` (#788).
#[tokio::test]
async fn stdio_tools_list_reports_the_35_tool_registry() {
    let mut child = spawn_stdio_server();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));

    handshake(&mut stdin, &mut reader).await;

    let list = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    send(&mut stdin, &list).await;

    let response = read_json_line(&mut reader).await;
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools/list result must carry a tools array");
    assert_eq!(tools.len(), 36, "the registry exposes all 36 tools");
    assert!(
        tools.iter().any(|t| t["name"] == "extract_domain"),
        "the tool registry must include extract_domain"
    );
    assert!(
        tools
            .iter()
            .any(|t| t["name"] == "get_accessibility_snapshot"),
        "the tool registry must include get_accessibility_snapshot"
    );
}

/// `tools/call extract_domain` succeeds end-to-end over stdio.
///
/// `extract_domain` is pure URL string logic — no network — so this stays
/// deterministic by construction.
#[tokio::test]
async fn stdio_tools_call_extract_domain_succeeds() {
    let mut child = spawn_stdio_server();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));

    handshake(&mut stdin, &mut reader).await;

    let call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "extract_domain",
            "arguments": {"url": "https://rust-lang.org"}
        }
    });
    send(&mut stdin, &call).await;

    let response = read_json_line(&mut reader).await;
    let result = &response["result"];
    assert!(
        !result["isError"].as_bool().unwrap_or(false),
        "extract_domain must not error; got: {response}"
    );
    let text = result["content"][0]["text"]
        .as_str()
        .expect("tool call content[0] must be a text block");
    assert!(
        text.contains("rust-lang.org"),
        "extract_domain must return the host; got: {text}"
    );
}

/// Dropping stdin (EOF) makes the server exit cleanly with code 0.
#[tokio::test]
async fn stdio_server_exits_cleanly_on_stdin_eof() {
    let mut child = spawn_stdio_server();
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));

    handshake(&mut stdin, &mut reader).await;

    // Close the JSON-RPC channel: stdin EOF ends the session.
    drop(stdin);

    let status = tokio::time::timeout(EXIT_TIMEOUT, child.wait())
        .await
        .expect("server exited after stdin EOF (rmcp closes on EOF)")
        .expect("reap the server child process");
    assert!(
        status.success(),
        "server must exit with code 0 on stdin EOF; got: {status}"
    );
}

/// Regression guard for #1108: stdin EOF *before* the handshake used to abort
/// the process at the `serve().expect()` site — exit 101 with a panic
/// backtrace aimed at the MCP client. The binary must instead log a clean
/// error and exit with the I/O error code (74), with no panic text on stderr.
#[tokio::test]
async fn stdio_server_exits_gracefully_on_pre_handshake_stdin_eof() {
    let mut child = spawn_stdio_server_with_stderr();
    // Close stdin before any JSON-RPC: serve() fails with ConnectionClosed.
    drop(child.stdin.take().expect("piped stdin"));
    // Nobody reads stdout; drop it so the child can never block on the pipe.
    drop(child.stdout.take().expect("piped stdout"));

    let status = wait_exited(&mut child).await;
    let stderr = drain_stderr(&mut child).await;

    assert!(
        !stderr.contains("panicked at"),
        "transport failure must not surface a panic backtrace to the MCP client; stderr:\n{stderr}"
    );
    assert_eq!(
        status.code(),
        Some(74),
        "transport failure must exit with the I/O error code (74); stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("Error:"),
        "stderr must carry the user-facing error line; got:\n{stderr}"
    );
}

/// Regression guard for #1108: a broken stdout pipe (client closes the read
/// end mid-handshake) used to panic at the same `serve().expect()` site with
/// `TransportError { BrokenPipe }`. The binary must exit cleanly instead.
#[tokio::test]
async fn stdio_server_exits_gracefully_on_broken_stdout_pipe() {
    let mut child = spawn_stdio_server_with_stderr();
    let mut stdin = child.stdin.take().expect("piped stdin");
    // Close the read end of the stdout pipe BEFORE the first server write:
    // sending the initialize response fails with EPIPE.
    drop(child.stdout.take().expect("piped stdout"));

    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "broken-pipe-test", "version": "1"}
        }
    });
    send(&mut stdin, &initialize).await;

    let status = wait_exited(&mut child).await;
    let stderr = drain_stderr(&mut child).await;

    assert!(
        !stderr.contains("panicked at"),
        "broken pipe must not surface a panic backtrace to the MCP client; stderr:\n{stderr}"
    );
    assert_eq!(
        status.code(),
        Some(74),
        "broken pipe must exit with the I/O error code (74); stderr:\n{stderr}"
    );
}
