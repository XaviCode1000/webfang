//! MCP server over stdio transport.
//!
//! This is the correct transport for MCP clients that launch the server
//! as a subprocess (OpenCode, Claude Desktop, Cursor, Cline, etc.).
//!
//! Run:
//!   cargo run --example mcp_server_stdio --quiet
//!
//! CRITICAL: All logging MUST go to stderr, never stdout.
//! The stdio transport uses stdout exclusively for JSON-RPC messages.
//! Any output to stdout (println!, tracing to stdout, etc.) will
//! corrupt the protocol and cause "Connection closed" errors.

use rmcp::service::ServiceExt;
use webfang::config::Config;
use webfang::di::Container;
use webfang::infrastructure::mcp_server::{McpHandler, McpState};

#[tokio::main]
async fn main() {
    // All logging to stderr — stdout is reserved for JSON-RPC
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let config = Config::default();
    let container = Container::new(config)
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
