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
//!
//! The last section (#1626, PC-3) covers contained panics on this transport:
//! a panic reaching the server from the wire must be caught, recorded with its
//! LOCATION by the shared panic hook, and leave the session usable. Its trigger
//! is the env-gated `test_panic_probe` tool, spawned only for that one test —
//! every other spawn here runs with the switch off, which is what keeps the
//! registry-size assertion at 36 honest.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStderr, ChildStdout};

/// Per-read timeout: generous enough for cold starts + BoringSSL init.
const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// Timeout for the graceful-shutdown wait on stdin EOF.
const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

/// Environment variable that switches the test-only panic probe on in the
/// CHILD process (#1626, PC-3). Restated here, not imported: this file asserts
/// the wire contract, so the literal name is the thing under test.
const PANIC_PROBE_ENV: &str = "WEBFANG_MCP_TEST_PANIC_TOOL";

/// Wire name of the env-gated probe tool. Unmistakably test-only on sight in a
/// `tools/list` dump, which is the point: if a report ever contains it, the
/// switch was set.
const PANIC_PROBE_TOOL: &str = "test_panic_probe";

/// Panic message the probe raises. Unique enough that a real panic cannot
/// produce it by accident, so the stderr assertion below cannot pass for the
/// wrong reason.
const PANIC_PROBE_MESSAGE: &str = "webfang-pc3-panic-probe: deliberate tool panic";

/// A cheap, network-free tool used as the "the session is still alive" probe —
/// pure URL string logic, so it is deterministic by construction.
const SURVIVAL_TOOL: &str = "extract_domain";

/// Per-frame byte cap the CHILD enforces on stdin (#1611 F7). Restated here,
/// not imported, for the reason above: this suite asserts the wire contract,
/// and the wire contract is "a frame larger than this is refused". Keeping the
/// literal means a silent change to the binary's `MAX_STDIN_LINE_BYTES` fails
/// here instead of being rubber-stamped by a shared constant.
const STDIN_FRAME_CAP: usize = 1_048_576;

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

/// Spawn with stderr piped, so #1108 failure-mode tests can assert the ABSENCE
/// of a panic backtrace on the child's stderr, and so #1626 PC-3 can assert the
/// structured record a contained panic leaves behind.
///
/// `panic_probe` is the env switch for the test-only probe tool: when `false`
/// the child's advertised tool surface is exactly what it was before #1626
/// (`stdin` is inherited, so the child cannot accidentally read the test's own
/// console).
fn spawn_stdio_server_with_stderr(panic_probe: bool) -> tokio::process::Child {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_webfang-mcp-stdio"));
    cmd.arg("--enable-ai")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if panic_probe {
        // Any non-empty value enables the probe — presence is the switch.
        cmd.env(PANIC_PROBE_ENV, "1");
    }
    cmd.spawn().expect("spawn the webfang-mcp-stdio binary")
}

/// Read the child's stderr to EOF on a background task.
///
/// The drain has to run while the session is still LIVE: a child whose stderr
/// pipe fills up blocks on the write, which is indistinguishable from a hang.
fn spawn_stderr_drain(mut stderr: ChildStderr) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf)
            .await
            .expect("read the child's stderr to EOF");
        String::from_utf8_lossy(&buf).into_owned()
    })
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
    let mut child = spawn_stdio_server_with_stderr(false);
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

/// Regression guard for #1151: closing the read end of stdout AFTER a
/// successful handshake used to hang the server forever — rmcp swallows the
/// post-handshake write error, so `server.waiting()` never resolved: no exit
/// code, no log.
///
/// The test bounds its own wait ([`EXIT_TIMEOUT`] via [`wait_exited`]): if the
/// bug is still present the child never exits and the test FAILS on the
/// timeout instead of hanging the suite (`kill_on_drop` reaps the orphan).
/// stderr is drained concurrently so a full log pipe can never mimic the
/// hang by blocking the child on a write.
#[tokio::test]
async fn issue_1151_post_handshake_stdout_close_exits_cleanly() {
    let mut child = spawn_stdio_server_with_stderr(false);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stderr_drain = tokio::spawn(async move {
        let mut buf = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf)
            .await
            .expect("read the child's stderr to EOF");
        String::from_utf8_lossy(&buf).into_owned()
    });

    handshake(&mut stdin, &mut reader).await;

    // Post-handshake transport death: the client closes its read end of
    // stdout. Dropping the only `ChildStdout` closes the fd, so the next
    // server write fails with EPIPE (the Rust runtime ignores SIGPIPE, hence
    // the failure surfaces as `Err`, not a signal).
    drop(reader);

    // Force a server→client write: the response to this request is what hits
    // the closed pipe. Without the fix nothing is ever observed and the
    // server pends in `waiting()` forever.
    let list = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    send(&mut stdin, &list).await;

    let status = wait_exited(&mut child).await;
    let stderr = stderr_drain
        .await
        .expect("the stderr drain task completes once the child exits");

    assert!(
        !stderr.contains("panicked at"),
        "transport death must not surface a panic backtrace; stderr:\n{stderr}"
    );
    assert_eq!(
        status.code(),
        Some(74),
        "post-handshake stdout death must exit with the I/O error code (74); stderr:\n{stderr}"
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
    let mut child = spawn_stdio_server_with_stderr(false);
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

// ===========================================================================
// Per-frame admission cap on stdin (#1611, F7)
// ===========================================================================

/// The read side of the transport was unbounded: rmcp's stdio transport reads
/// with `read_until(b'\n', &mut line_buf)` over an unbounded `Vec<u8>`
/// (`rmcp-1.8.0/src/transport/async_rw.rs:125-133`, `:52`), so a peer decides
/// how much memory the server allocates before a byte is parsed. The binary now
/// hands `serve()` a bounded reader instead of `tokio::io::stdin()`.
///
/// What this pins, end to end through the real binary:
/// 1. the session is ALIVE first (a full handshake), so the refusal cannot be
///    confused with a boot failure;
/// 2. an oversize frame is refused — the payload here is VALID JSON of the
///    oversize kind, so this is a size refusal, not rmcp's parse error;
/// 3. it leaves a non-zero exit and a structured record on stderr, because
///    rmcp's own reaction (log line + session end) is indistinguishable from a
///    client hangup — the whole reason the refusal is observable here at all.
#[tokio::test]
async fn stdio_oversize_stdin_frame_is_refused_with_a_visible_reason() {
    let mut child = spawn_stdio_server_with_stderr(false);
    let mut stdin = child.stdin.take().expect("piped stdin");
    // Read end stays OPEN for the whole test: dropping it would make the
    // child's writes fail with EPIPE and the refusal could be reported through
    // the stdout-death path instead. Nothing is read after the handshake — the
    // refused frame is never dispatched, so the child has nothing to say.
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let stderr_drain = spawn_stderr_drain(child.stderr.take().expect("piped stderr"));

    handshake(&mut stdin, &mut reader).await;

    // One valid JSON-RPC line, past the cap. The padding rides in an
    // `arguments` key the tool would never read: the frame must be refused on
    // its SIZE, before any parsing or dispatch.
    let padding = "x".repeat(STDIN_FRAME_CAP + 4096);
    let frame = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"tools/call\",\"params\":{{\
         \"name\":\"scrape_url\",\"arguments\":{{\"url\":\"https://example.com/\",\
         \"pad\":\"{padding}\"}}}}}}\n"
    );
    assert!(
        frame.len() > STDIN_FRAME_CAP,
        "the payload must really exceed the cap under test; got {} bytes",
        frame.len()
    );

    // The child refuses the frame and exits WHILE this write is still in
    // flight, so a broken pipe is the expected outcome, not a failure. A
    // timeout is NOT expected: the server is reading.
    tokio::time::timeout(READ_TIMEOUT, async {
        let _ = stdin.write_all(frame.as_bytes()).await;
        let _ = stdin.flush().await;
    })
    .await
    .expect("the child kept draining its stdin until it refused the frame");

    let status = wait_exited(&mut child).await;
    let stderr = stderr_drain
        .await
        .expect("the stderr drain task completes once the child exits");

    assert!(
        !stderr.contains("panicked at"),
        "a cap refusal must not surface a panic backtrace; stderr:\n{stderr}"
    );
    assert_eq!(
        status.code(),
        Some(74),
        "an oversize frame must exit with the I/O error code (74); stderr:\n{stderr}"
    );
    // This assertion is STRUCTURAL, not probabilistic. It used to depend on
    // `select!` happening to pick the stdin-death arm over `server.waiting()`:
    // rmcp resolves `waiting()` for a read error just as it does for EOF, so
    // both arms were ready and the pick was random. The stdin signal is now
    // checked before the select's outcome is destructured, so the refusal is
    // reported no matter which branch won (R3-STDIN-SELECT-RACE).
    // The operator-facing line, same shape as every other transport death.
    assert!(
        stderr.contains("Error:"),
        "stderr must carry the user-facing error line; got:\n{stderr}"
    );
    // The structured record, matched as independent substrings for the reason
    // the panic-probe test above documents: the `fmt()` layout is not a
    // contract, the event and the field NAME are.
    for expected in [
        "mcp stdio stdin refused: input frame exceeded the per-line cap", // the event
        "refused_at_bytes",                                               // ...and where it stopped
        "stdin cap", // the reason main() reports
    ] {
        assert!(
            stderr.contains(expected),
            "child stderr must contain {expected:?} after refusing an oversize \
             frame; stderr:\n{stderr}"
        );
    }
}

// ===========================================================================
// Contained panic on stdio — transport parity for the panic hook (#1626, PC-3)
// ===========================================================================

/// #1626 PC-3: a contained panic on the stdio transport must leave the SAME
/// structured record the HTTP transport owes the operator, and must not take
/// the session down with it.
///
/// The asymmetry this closes: `setup_panic_hook()` was installed only by
/// `start_mcp_server` (the HTTP transport), so a contained panic over stdio
/// logged the payload — and never the panic LOCATION, which is exactly what an
/// operator needs to find the bug. Both transports now owe the same record, and
/// this test drives the real `webfang-mcp-stdio` binary end to end: the panic
/// comes from the wire (the env-gated `test_panic_probe` tool), not from an
/// in-crate seam an integration test cannot see.
///
/// The four assertions are the four halves of the contract:
/// 1. the env switch reached the child (the probe is advertised);
/// 2. the panicking call is CONTAINED — `isError`, plain explanation, and the
///    payload withheld from the client (a dead transport answers nothing, and
///    `read_json_line` would time out instead);
/// 3. the child's stderr carries the hook's structured record, message AND
///    location;
/// 4. a FOLLOWING call on the SAME session still succeeds — the F2 promise
///    that a contained panic restores the session.
#[tokio::test]
async fn stdio_contained_panic_is_recorded_with_location_and_keeps_the_session() {
    // Arrange
    let mut child = spawn_stdio_server_with_stderr(true);
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut reader = BufReader::new(child.stdout.take().expect("piped stdout"));
    let stderr_drain = spawn_stderr_drain(child.stderr.take().expect("piped stderr"));

    handshake(&mut stdin, &mut reader).await;

    // Act: list the registry. Reaching this point already proves the handshake
    // was answered, so the only new thing asserted here is the env switch.
    let list = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    send(&mut stdin, &list).await;
    let response = read_json_line(&mut reader).await;
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools/list result must carry a tools array");
    assert!(
        tools.iter().any(|t| t["name"] == PANIC_PROBE_TOOL),
        "{PANIC_PROBE_ENV} must register the probe tool in the child; got {} tools",
        tools.len()
    );

    // Act: call it. The panic is raised here and contained by the existing
    // `catch_unwind` in `McpHandler::call_tool`.
    let panic_call = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {"name": PANIC_PROBE_TOOL, "arguments": {}}
    });
    send(&mut stdin, &panic_call).await;

    // Assert: containment, not a dead transport.
    let response = read_json_line(&mut reader).await;
    let result = &response["result"];
    assert_eq!(
        result["isError"], true,
        "a contained panic must answer a normal tool error; got: {response}"
    );
    let explanation = result["content"][0]["text"]
        .as_str()
        .expect("tool call content[0] must be a text block");
    assert!(
        explanation.contains("fue contenida"),
        "the caller must get the plain containment explanation; got: {explanation}"
    );
    assert!(
        !explanation.contains(PANIC_PROBE_MESSAGE),
        "the panic payload must never reach the client; got: {explanation}"
    );

    // Act: the F2 promise — a following call on the SAME session works.
    let survivor = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": SURVIVAL_TOOL,
            "arguments": {"url": "https://rust-lang.org/after-panic"}
        }
    });
    send(&mut stdin, &survivor).await;
    let response = read_json_line(&mut reader).await;
    let result = &response["result"];
    assert!(
        !result["isError"].as_bool().unwrap_or(false),
        "the session must still be dispatching after a contained panic; got: {response}"
    );
    let text = result["content"][0]["text"]
        .as_str()
        .expect("tool call content[0] must be a text block");
    assert!(
        text.contains("rust-lang.org"),
        "the surviving call must return its real answer; got: {text}"
    );

    // Act: close the channel so the child exits and its stderr reaches EOF.
    drop(stdin);
    let status = wait_exited(&mut child).await;
    let stderr = stderr_drain
        .await
        .expect("the stderr drain task completes once the child exits");

    // Assert: the panic hook's structured record. Matched as independent
    // substrings, never as a rendered line — the `fmt()` layout, its ANSI
    // styling, and even its line breaks are not a stable contract, and the
    // record's `panic.message` value embeds a newline of its own. What IS a
    // contract: `server panicked` and the `panic.location` field name are
    // emitted by `panic_hook::setup_panic_hook` and by nothing else, and the
    // location names the probe because that is where the panic is raised.
    for expected in [
        "server panicked",   // the hook's tracing event
        "panic.location",    // ...carrying the panic LOCATION...
        "test_probe.rs:",    // ...which names the probe's source file
        PANIC_PROBE_MESSAGE, // the panic message itself
    ] {
        assert!(
            stderr.contains(expected),
            "child stderr must contain {expected:?} after a contained panic; stderr:\n{stderr}"
        );
    }
    assert!(
        status.success(),
        "a contained panic must not change the exit code; got: {status}\nstderr:\n{stderr}"
    );
}
