//! MCP Server — Stdio transport
//!
//! Launches the webfang MCP server over stdin/stdout for clients that spawn
//! the server as a subprocess (OpenCode, Claude Desktop, Cline, etc.).
//!
//! ## Quick start
//!
//! ```bash
//! cargo run -p rust_scraper_mcp --example mcp_server_stdio
//! ```
//!
//! ## Protocol
//!
//! - **stdin** → JSON-RPC messages (one per line, newline-delimited)
//! - **stdout** → JSON-RPC responses (one per line)
//! - **stderr** → logs only (tracing, panics)
//!
//! ⚠️ **CRITICAL**: All output to stdout corrupts the JSON-RPC protocol.
//! Never use `println!` or route tracing to stdout.
//!
//! ## Session flow (no session ID needed)
//!
//! Unlike the HTTP transport, stdio maintains state across messages within
//! the same process. No `Mcp-Session-Id` header required.
//!
//! ```text
//! → {"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}
//! ← {"jsonrpc":"2.0","id":1,"result":{...}}
//!
//! → {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
//! ← (no response — notification)
//!
//! → {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
//! ← {"jsonrpc":"2.0","id":2,"result":{"tools":[...]}}
//!
//! → {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{ \
//!     "name":"validate_url","arguments":{"url":"https://example.com"} \
//!   }}
//! ← {"jsonrpc":"2.0","id":3,"result":{"content":[...]}}
//! ```
//!
//! ## Testing with a script
//!
//! ```bash
//! # Pipe all messages at once (newline-delimited)
//! echo '
//! {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}
//! {"jsonrpc":"2.0","method":"notifications/initialized","params":{}}
//! {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
//! {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"validate_url","arguments":{"url":"https://example.com"}}}
//! ' | cargo run -p rust_scraper_mcp --example mcp_server_stdio 2>/dev/null
//! ```
//!
//! ## Testing with Python (interactive)
//!
//! ```python
//! import json, subprocess, select, time
//!
//! proc = subprocess.Popen(
//!     ['cargo', 'run', '-p', 'rust_scraper_mcp', '--example', 'mcp_server_stdio'],
//!     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE
//! )
//!
//! def send(msg):
//!     proc.stdin.write((json.dumps(msg) + '\n').encode())
//!     proc.stdin.flush()
//!
//! def recv(timeout=5):
//!     ready, _, _ = select.select([proc.stdout], [], [], timeout)
//!     if ready:
//!         line = proc.stdout.readline().decode().strip()
//!         if line:
//!             return json.loads(line)
//!     return None
//!
//! # Initialize
//! send({'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {
//!     'protocolVersion': '2025-03-26', 'capabilities': {},
//!     'clientInfo': {'name': 'test', 'version': '1.0'}
//! }})
//! resp = recv()
//! print(f"Server: {resp['result']['serverInfo']}")
//!
//! # Confirm
//! send({'jsonrpc': '2.0', 'method': 'notifications/initialized', 'params': {}})
//! time.sleep(0.3)
//!
//! # List tools
//! send({'jsonrpc': '2.0', 'id': 2, 'method': 'tools/list', 'params': {}})
//! resp = recv()
//! for tool in resp['result']['tools']:
//!     print(f"  - {tool['name']}")
//!
//! proc.terminate()
//! ```
//!
//! ## OpenCode / Claude Desktop config
//!
//! Add to your MCP client config:
//!
//! ```json
//! {
//!   "mcpServers": {
//!     "webfang": {
//!       "command": "cargo",
//!       "args": ["run", "-p", "rust_scraper_mcp", "--example", "mcp_server_stdio", "--release"],
//!       "cwd": "/path/to/rust_scraper"
//!     }
//!   }
//! }
//! ```
//!
//! ## HTTP transport (for remote clients)
//!
//! For HTTP-based MCP clients, use `mcp_server` instead:
//!
//! ```bash
//! cargo run -p rust_scraper_mcp --example mcp_server --release
//! ```
//!
//! See `mcp_server.rs` docs for full HTTP testing guide.

use rmcp::service::ServiceExt;
use rust_scraper_core::config::Config;
use rust_scraper_core::di::{Container, ContainerExt};
use rust_scraper_mcp::mcp_server::{McpHandler, McpState};

#[tokio::main]
async fn main() {
    // All logging to stderr — stdout is reserved for JSON-RPC
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let config = Config::default();
    let container = Container::from_config(config)
        .await
        .expect("failed to create container");
    let state = McpState::new(container);
    let handler = McpHandler::new(state);

    // Serve over stdio — stdin/stdout for JSON-RPC, stderr for logs
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let server = handler
        .serve(transport)
        .await
        .expect("failed to start MCP server over stdio");

    // Wait for the server to finish (client disconnects or stdin closes)
    server.waiting().await.expect("MCP server error");
}
