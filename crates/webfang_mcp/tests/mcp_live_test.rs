//! Live integration tests for the MCP server binary over stdio.
//!
//! **Manual validation only** — these tests are excluded from CI via `#[ignore]`
//! because they require a pre-built binary (`cargo build --release --example mcp_server_stdio`).
//! For automated regression testing, use `mcp_lifecycle_test.rs` instead, which
//! tests the same MCP session lifecycle over HTTP without needing a pre-built binary.
//!
//! Run manually:
//!   cargo nextest run --release --test-threads 1 mcp_live_test
//!   cargo test --release --test mcp_live_test -- --nocapture

use assert_cmd::Command;
use std::time::Duration;

/// MCP initialize handshake — required before any tool call.
/// Per the MCP spec, the first message MUST be `initialize`, then
/// the client sends a `notifications/initialized` notification.
const MCP_INIT: &str = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"webfang_test","version":"1.0"}}}"#;
const MCP_INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

/// Helper: send a JSON-RPC request and return parsed response.
///
/// Automatically performs the MCP initialize handshake first, then
/// sends the actual tool request. All messages are piped to stdin
/// at once; the server processes them sequentially and exits when
/// stdin closes.
fn send_request(cmd: &mut Command, request: &str) -> serde_json::Value {
    let input = format!("{MCP_INIT}\n{MCP_INITIALIZED}\n{request}\n");
    cmd.write_stdin(input);

    let output = cmd.output().expect("failed to get MCP output");
    assert!(
        output.status.success(),
        "MCP process exited with code {}",
        output.status
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // MCP sends multiple responses (initialize + tool result).
    // Find the tool response (any id that is NOT 0).
    for line in stdout.lines() {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
            if val.get("id").and_then(|id| id.as_u64()) != Some(0) {
                return val;
            }
        }
    }
    panic!(
        "No valid JSON-RPC tool response found in MCP output:\n{}",
        stdout.chars().take(500).collect::<String>()
    );
}

#[test]
#[ignore = "requires cargo build --release --example mcp_server_stdio"]
fn test_mcp_binary_exists() {
    // Just verify the binary exists and is executable
    let mut cmd = Command::cargo_bin("examples/mcp_server_stdio").expect(
        "MCP server binary not found — run 'cargo build --release --example mcp_server_stdio'",
    );
    cmd.timeout(Duration::from_secs(5));
    // Write and close stdin
    cmd.write_stdin("").assert().failure(); // fails because stdin closes before JSON-RPC
}

#[test]
#[ignore = "requires cargo build --release --example mcp_server_stdio"]
fn test_mcp_tools_list() {
    let mut cmd =
        Command::cargo_bin("examples/mcp_server_stdio").expect("MCP server binary not found");
    cmd.timeout(Duration::from_secs(10));

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let response = send_request(&mut cmd, request);

    // Verify response structure
    assert_eq!(response["id"], 1, "response id should match request id");

    // Check for tools array in result
    let has_tools = response["result"]["tools"].is_array() || response["result"].is_array();
    assert!(
        has_tools,
        "response should contain tools array: {}",
        response
    );

    let tools = if response["result"]["tools"].is_array() {
        response["result"]["tools"].as_array().unwrap()
    } else {
        response["result"].as_array().unwrap()
    };

    // Verify critical tools are present
    let tool_names: Vec<&str> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap_or(""))
        .collect();

    let required_tools = [
        "scrape_url",
        "clean_html",
        "convert_html_to_markdown",
        "validate_url",
        "extract_domain",
        "normalize_url",
        "detect_waf",
        "list_waf_providers",
        "export_file",
        "detect_obsidian_vault",
        "build_obsidian_uri",
        "highlight_code_blocks",
        "generate_frontmatter",
        "is_internal_link",
        "discover_urls",
    ];

    for tool in &required_tools {
        assert!(
            tool_names.contains(tool),
            "Tool '{}' should be registered but is missing from: {:?}",
            tool,
            tool_names
        );
    }

    // At minimum we should have the core tools
    assert!(
        tools.len() >= 20,
        "Expected at least 20 tools registered, got {}",
        tools.len()
    );
}

#[test]
#[ignore = "requires cargo build --release --example mcp_server_stdio"]
fn test_mcp_validate_url_tool() {
    let mut cmd =
        Command::cargo_bin("examples/mcp_server_stdio").expect("MCP server binary not found");
    cmd.timeout(Duration::from_secs(10));

    let request = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"validate_url","arguments":{"url":"https://example.com:8080/path?q=1#frag"}}}"#;
    let response = send_request(&mut cmd, request);

    let content = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        content.contains("valid"),
        "response should contain validity info"
    );
    assert!(
        content.contains("example.com"),
        "response should contain host"
    );
    assert!(content.contains("8080"), "response should contain port");
}

#[test]
#[ignore = "requires cargo build --release --example mcp_server_stdio"]
fn test_mcp_invalid_url_shows_error() {
    let mut cmd =
        Command::cargo_bin("examples/mcp_server_stdio").expect("MCP server binary not found");
    cmd.timeout(Duration::from_secs(10));

    let request = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"validate_url","arguments":{"url":"not-a-valid-url"}}}"#;
    let response = send_request(&mut cmd, request);

    let text = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        text.contains("valid"),
        "should return validity info even for invalid URLs"
    );
    assert!(text.contains("false"), "should mark as invalid");
}

#[test]
#[ignore = "requires cargo build --release --example mcp_server_stdio"]
fn test_mcp_clean_html_removes_scripts() {
    let mut cmd =
        Command::cargo_bin("examples/mcp_server_stdio").expect("MCP server binary not found");
    cmd.timeout(Duration::from_secs(10));

    let html = "<html><script>alert('xss')</script><body><p>Hello</p></body></html>";
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{{"name":"clean_html","arguments":{{"html":{}}}}}}}"#,
        serde_json::to_string(html).unwrap()
    );
    let response = send_request(&mut cmd, &request);
    let cleaned = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");

    assert!(
        !cleaned.contains("<script>"),
        "scripts should be removed: {}",
        cleaned
    );
    assert!(
        cleaned.contains("Hello"),
        "content should be preserved: {}",
        cleaned
    );
}

#[test]
#[ignore = "requires cargo build --release --example mcp_server_stdio"]
fn test_mcp_extract_domain() {
    let mut cmd =
        Command::cargo_bin("examples/mcp_server_stdio").expect("MCP server binary not found");
    cmd.timeout(Duration::from_secs(10));

    let request = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"extract_domain","arguments":{"url":"https://blog.example.com/docs"}}}"#;
    let response = send_request(&mut cmd, request);
    let domain = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");

    assert_eq!(
        domain, "blog.example.com",
        "should extract domain correctly"
    );
}

#[test]
#[ignore = "requires cargo build --release --example mcp_server_stdio"]
fn test_mcp_list_waf_providers() {
    let mut cmd =
        Command::cargo_bin("examples/mcp_server_stdio").expect("MCP server binary not found");
    cmd.timeout(Duration::from_secs(10));

    let request = r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"list_waf_providers","arguments":{}}}"#;
    let response = send_request(&mut cmd, request);
    let providers = response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");

    assert!(providers.contains("Cloudflare"), "should list Cloudflare");
    assert!(providers.contains("Akamai"), "should list Akamai");
    assert!(providers.contains("reCAPTCHA"), "should list reCAPTCHA");
}
