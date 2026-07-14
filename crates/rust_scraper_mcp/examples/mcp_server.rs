//! MCP server entry point.
//!
//! Launches the Streamable HTTP MCP server used by the CI `MCP handshake` job.
//! The job runs `cargo build --example mcp_server --all-features` and then probes
//! `http://127.0.0.1:8080/mcp` for a successful `initialize` response.

use std::net::SocketAddr;

use anyhow::Result;
use rust_scraper_core::config::Config;
use rust_scraper_core::di::{Container, ContainerExt};
use rust_scraper_mcp::mcp_server::server::{start_mcp_server, DEFAULT_MCP_ADDR};
use rust_scraper_mcp::mcp_server::McpState;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::default();
    let container = Container::from_config(config)
        .await
        .map_err(|e| anyhow::anyhow!("failed to build container: {e}"))?;
    let state = McpState::new(container);
    let addr: SocketAddr = DEFAULT_MCP_ADDR.parse()?;
    start_mcp_server(state, addr).await
}
