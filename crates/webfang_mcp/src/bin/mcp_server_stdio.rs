//! MCP Server — Stdio transport (binary entry point).
//!
//! Launches the webfang MCP server over stdin/stdout for clients that spawn
//! the server as a subprocess (OpenCode, Claude Desktop, Cline, etc.). This
//! replaces the old `examples/mcp_server_stdio.rs` example.

use clap::Parser;
use rmcp::service::ServiceExt;
use webfang_core::config::Config;
use webfang_core::di::{Container, ContainerExt};
use webfang_mcp::mcp_server::{McpHandler, McpState};

/// Webfang MCP Server — Stdio transport.
#[derive(Parser, Debug)]
#[command(
    name = "webfang-mcp-stdio",
    version,
    about = "Webfang MCP Server (stdio transport)",
    long_about = "Exposes 35 scraper tools via the Model Context Protocol over stdin/stdout."
)]
struct Args {
    /// Enable AI semantic cleaning (requires the `ai` feature at build time).
    #[arg(long, env = "WEBFANG_MCP_AI")]
    enable_ai: bool,
}

#[tokio::main]
async fn main() {
    // All logging to stderr — stdout is reserved for JSON-RPC.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let config = Config::default();
    let container = Container::from_config(config)
        .await
        .expect("failed to create container");

    // REQ-07: construct and inject the AI semantic cleaner behind the `ai`
    // feature, mirroring the HTTP binary. Construction is OPT-IN via the
    // `--enable-ai` flag. The block shadows `container` with a cleaner-backed
    // one; shadowing (not `mut`) keeps the off-build warning-free.
    #[cfg(feature = "ai")]
    let container = if args.enable_ai {
        let variant = webfang_ai::AiModel::from_env_or_default();
        let model_config = webfang_ai::ModelConfig::default().with_model_variant(variant);
        match webfang_ai::SemanticCleanerImpl::new(model_config).await {
            Ok(cleaner) => {
                let (pool, tokenizer) = cleaner.shared_inference();
                let container = container.with_cleaner(std::sync::Arc::new(cleaner));
                webfang_mcp::mcp_server::ai_wiring::wire_ai_ports(container, pool, tokenizer).await
            },
            Err(e) => {
                tracing::warn!("semantic cleaner unavailable, continuing without AI: {e}");
                container
            },
        }
    } else {
        container
    };

    // Keep the `enable_ai` flag honest when compiled without the `ai` feature.
    #[cfg(not(feature = "ai"))]
    if args.enable_ai {
        tracing::warn!("--enable-ai requested but the `ai` feature is not compiled in; ignoring");
    }

    let state = McpState::new(container);
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
