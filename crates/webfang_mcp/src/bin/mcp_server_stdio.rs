//! MCP Server — Stdio transport (binary entry point).
//!
//! Launches the webfang MCP server over stdin/stdout for clients that spawn
//! the server as a subprocess (OpenCode, Claude Desktop, Cline, etc.). This
//! replaces the old `examples/mcp_server_stdio.rs` example.

use std::sync::Arc;

use clap::Parser;
use rmcp::service::ServiceExt;
use webfang_core::cli::error::CliExit;
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
async fn main() -> CliExit {
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
    // A construction failure is a boot-time error: log it (English, structured)
    // and exit with the config-error code — never a panic backtrace (#1123).
    let container = match build_container().await {
        Ok(container) => Arc::new(container),
        Err(e) => {
            tracing::error!(error = %e, "MCP stdio boot failed: container construction");
            return CliExit::ConfigError(format!(
                "No se pudo crear el contenedor del servidor MCP: {e}"
            ));
        },
    };

    if args.enable_ai {
        spawn_ai_wiring(Arc::clone(&container));
    }

    // Keep a second handle: `serve()` moves the handler (and with it the
    // state and its container) into the service, so this clone is what lets
    // main drain the crawl-result writer at exit (#1143 review).
    let exit_container = Arc::clone(&container);

    let state = McpState::from_container(container).with_export_roots(args.export_roots);

    let handler = McpHandler::new(state);

    // Serve over stdio — stdin/stdout for JSON-RPC, stderr for logs.
    // A closed stdin or broken pipe before the handshake completes fails
    // `serve()`; panicking there would send a backtrace to the spawning MCP
    // client (OpenCode, Claude Desktop, …). Log and exit with the I/O error
    // code instead (#1108).
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let server = match handler.serve(transport).await {
        Ok(server) => server,
        Err(e) => {
            tracing::error!(error = %e, "mcp stdio serve failed");
            return CliExit::IoError(format!("No se pudo iniciar el servidor MCP por stdio: {e}"));
        },
    };

    // Wait for the server to finish (client disconnects or stdin closes).
    let waiting_result = server.waiting().await;

    // #1121: same drain as the HTTP transport (server.rs) — the stdio tools
    // persist crawl results through the very same background writer, so on
    // this transport too `shutdown()` must join it before the runtime goes
    // away. Runs even if `waiting()` errored; its error is propagated after.
    if let Some(repo) = exit_container.crawl_result_repository() {
        if let Err(e) = repo.shutdown().await {
            tracing::warn!(error = %e, "crawl-result writer shutdown reported errors");
        }
    }

    // Normal EOF shutdown returns Ok → exit 0; a transport error mid-session
    // gets the same clean log + exit treatment instead of a panic backtrace
    // aimed at the spawning MCP client (#1108).
    if let Err(e) = waiting_result {
        tracing::error!(error = %e, "mcp server terminated with error");
        return CliExit::IoError(format!("El servidor MCP por stdio terminó con error: {e}"));
    }

    CliExit::Success
}
