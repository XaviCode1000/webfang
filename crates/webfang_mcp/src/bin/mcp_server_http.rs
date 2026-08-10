//! MCP Server — Streamable HTTP transport (binary entry point).
//!
//! Launches the webfang MCP server over HTTP on `127.0.0.1:8080/mcp` by
//! default, with full clap-based configuration (`--help` for all flags).
//! This replaces the old `examples/mcp_server.rs` example.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use webfang_core::adapters::downloader::{DownloadConfig, Downloader};
use webfang_core::config::Config;
use webfang_core::di::{Container, ContainerExt};
use webfang_mcp::mcp_server::server::{start_mcp_server, ServerOptions, DEFAULT_MCP_ADDR};
use webfang_mcp::mcp_server::McpState;

/// Webfang MCP Server — Streamable HTTP transport.
#[derive(Parser, Debug)]
#[command(
    name = "webfang-mcp",
    version,
    about = "Webfang MCP Server (HTTP transport)",
    long_about = "Exposes 35 scraper tools via the Model Context Protocol over Streamable HTTP."
)]
struct Args {
    /// Bind address (host:port) for the MCP server.
    #[arg(long, env = "WEBFANG_MCP_BIND", default_value = DEFAULT_MCP_ADDR)]
    bind: SocketAddr,

    /// Request timeout in seconds.
    #[arg(long, env = "WEBFANG_MCP_TIMEOUT_SECS", default_value_t = 30)]
    timeout_secs: u64,

    /// Max request body size in bytes.
    #[arg(long, env = "WEBFANG_MCP_BODY_LIMIT", default_value_t = 10_485_760)]
    body_limit: usize,

    /// Rate limit: requests per second.
    #[arg(long, env = "WEBFANG_MCP_RATE", default_value_t = 10)]
    rate: u32,

    /// Rate limit: burst size.
    #[arg(long, env = "WEBFANG_MCP_BURST", default_value_t = 20)]
    burst: u32,

    /// Auth token; if set, requires `Authorization: Bearer <token>`.
    #[arg(long, env = "WEBFANG_MCP_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Enable AI semantic cleaning (requires the `ai` feature at build time).
    #[arg(long, env = "WEBFANG_MCP_AI")]
    enable_ai: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let config = Config::default();
    let container = Container::from_config(config)
        .await
        .map_err(|e| anyhow::anyhow!("failed to build container: {e}"))?;

    // REQ-07: construct and inject the AI semantic cleaner behind the `ai`
    // feature, mirroring the CLI (main.rs). Construction is OPT-IN via the
    // `--enable-ai` flag so a plain `cargo run` smoke run does not download
    // the ~390 MB ONNX model at startup. The block shadows `container` with a
    // cleaner-backed one; shadowing (not `mut`) keeps the off-build warning-free.
    #[cfg(feature = "ai")]
    let container = if args.enable_ai {
        let variant = webfang_ai::AiModel::from_env_or_default();
        let model_config = webfang_ai::ModelConfig::default().with_model_variant(variant);
        // Wire the semantic cleaner (clean_html / semantic_cleaner tools), then
        // share its ONNX pool + tokenizer with the vault-search ports (#433) so
        // the model is loaded exactly once. The vault ports are wired only when
        // the cleaner succeeds — they reuse the components it resolved.
        match webfang_ai::SemanticCleanerImpl::new(model_config).await {
            Ok(cleaner) => {
                let (pool, tokenizer) = cleaner.shared_inference();
                let container = container.with_cleaner(Arc::new(cleaner));
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

    // Inject a shared Downloader so `download_assets` reuses one connection
    // pool across tool calls. The default config writes to `./downloads`
    // relative to the working directory.
    let state = McpState::new(container)
        .with_downloader(Arc::new(Downloader::new(DownloadConfig::default())?));

    let opts = ServerOptions {
        request_timeout_secs: args.timeout_secs,
        body_limit_bytes: args.body_limit,
        rate_per_second: args.rate,
        rate_burst: args.burst,
        auth_token: args.auth_token,
    };

    start_mcp_server(state, args.bind, opts).await
}
