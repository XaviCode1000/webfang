//! MCP Server — Stdio transport (binary entry point).
//!
//! Launches the webfang MCP server over stdin/stdout for clients that spawn
//! the server as a subprocess (OpenCode, Claude Desktop, Cline, etc.). This
//! replaces the old `examples/mcp_server_stdio.rs` example.

use std::sync::Arc;

use clap::Parser;
use rmcp::service::ServiceExt;
use webfang_mcp::mcp_server::{build_container, spawn_ai_wiring, McpHandler, McpState};

/// Webfang MCP Server — Stdio transport.
#[derive(Parser, Debug)]
#[command(
    name = "webfang-mcp-stdio",
    version,
    about = "Webfang MCP Server (stdio transport)",
    long_about = "Exposes 36 tools via the Model Context Protocol over stdin/stdout."
)]
struct Args {
    /// Enable AI semantic cleaning (requires the `ai` feature at build time).
    #[arg(long, env = "WEBFANG_MCP_AI")]
    enable_ai: bool,

    /// Allowed root directories for absolute `output_dir` paths (#696).
    /// Repeatable or comma-separated. When omitted, absolute `output_dir`
    /// values are rejected (fail-closed); relative paths always work.
    #[arg(long, env = "WEBFANG_MCP_EXPORT_ROOTS", value_delimiter = ',')]
    export_roots: Vec<std::path::PathBuf>,
}

#[tokio::main]
async fn main() {
    // All logging to stderr — stdout is reserved for JSON-RPC.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    // Keep the `enable_ai` flag honest when compiled without the `ai` feature.
    #[cfg(not(feature = "ai"))]
    if args.enable_ai {
        tracing::warn!("--enable-ai requested but the `ai` feature is not compiled in; ignoring");
    }

    // Build the container FAST — no model resolution happens here (#759).
    // The AI ports are wired lazily in a background task after the server
    // starts serving, so the MCP `initialize` handshake is never blocked
    // behind the hf_hub model resolution (~390 MB on a cold cache).
    let container = Arc::new(build_container().await);

    if args.enable_ai {
        spawn_ai_wiring(Arc::clone(&container));
    }

    let state = McpState::from_container(container).with_export_roots(args.export_roots);

    let handler = McpHandler::new(state);

    // Serve over stdio — stdin/stdout for JSON-RPC, stderr for logs.
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let server = handler
        .serve(transport)
        .await
        .expect("failed to start MCP server over stdio");

    // Wait for the server to finish (client disconnects or stdin closes).
    server.waiting().await.expect("MCP server error");
}
