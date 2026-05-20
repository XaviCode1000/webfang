//! MCP Server — Axum router with Streamable HTTP transport
//!
//! Sets up the MCP server using rmcp's StreamableHttpService
//! mounted on an Axum router at /mcp.

use std::net::SocketAddr;

use axum::Router;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager,
    tower::StreamableHttpService,
};
use tracing::info;

use super::state::McpState;
use super::McpHandler;

/// Default address for the MCP server.
pub const DEFAULT_MCP_ADDR: &str = "127.0.0.1:8080";

/// Build the Axum router with MCP endpoint.
pub fn build_mcp_router(state: McpState) -> Router {
    let service = StreamableHttpService::new(
        move || Ok(McpHandler::new(state.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    Router::new()
        .nest_service("/mcp", service)
}

/// Start the MCP server on the given address.
pub async fn start_mcp_server(state: McpState, addr: SocketAddr) -> anyhow::Result<()> {
    let app = build_mcp_router(state);

    info!("MCP server starting on http://{}/mcp", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Wait for Ctrl+C and return.
async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install Ctrl+C handler");
    info!("MCP server shutting down");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::di::Container;
    use crate::config::Config;

    #[tokio::test]
    async fn test_build_mcp_router_does_not_panic() {
        // Minimal config for test
        let config = Config::default();
        let container = Container::new(config).await.unwrap();
        let state = McpState::new(container);
        let _router = build_mcp_router(state);
        // If we get here, router built successfully
    }
}
