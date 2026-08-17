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
use webfang_mcp::mcp_server::server::{
    require_auth_for_external_bind, start_mcp_server, ServerOptions, DEFAULT_MCP_ADDR,
};
use webfang_mcp::mcp_server::{build_container_with_ai, McpState};

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

    /// Allowed root directories for absolute `output_dir` paths (#696).
    /// Repeatable or comma-separated. When omitted, absolute `output_dir`
    /// values are rejected (fail-closed); relative paths always work.
    #[arg(long, env = "WEBFANG_MCP_EXPORT_ROOTS", value_delimiter = ',')]
    export_roots: Vec<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    // Keep the `enable_ai` flag honest when compiled without the `ai` feature.
    #[cfg(not(feature = "ai"))]
    if args.enable_ai {
        tracing::warn!("--enable-ai requested but the `ai` feature is not compiled in; ignoring");
    }

    // REQ-06: fail fast on a tokenless non-loopback bind, before building any
    // container/downloader. Loopback binds stay token-free (development mode).
    require_auth_for_external_bind(args.bind, args.auth_token.is_some())?;
    if args.bind.ip().is_loopback() && args.auth_token.is_none() {
        tracing::warn!("MCP server starting on loopback without auth token (development mode)");
    }

    // Build container with optional AI wiring (shared with mcp_server_stdio.rs)
    let container = build_container_with_ai(args.enable_ai).await;

    // Inject a shared Downloader so `download_assets` reuses one connection
    // pool across tool calls. The default config writes to `./downloads`
    // relative to the working directory.
    let state = McpState::new(container)
        .with_downloader(Arc::new(Downloader::new(DownloadConfig::default())?))
        .with_export_roots(args.export_roots);

    let opts = ServerOptions {
        request_timeout_secs: args.timeout_secs,
        body_limit_bytes: args.body_limit,
        rate_per_second: args.rate,
        rate_burst: args.burst,
        auth_token: args.auth_token,
    };

    // Disable SSRF for testing with env var, otherwise use default (enabled)
    if std::env::var("WEBFANG_MCP_DISABLE_SSRF").is_ok() {
        tracing::debug!("SSRF protection disabled (test mode)");
    }

    start_mcp_server(state, args.bind, opts).await
}
