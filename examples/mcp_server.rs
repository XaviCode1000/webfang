//! Standalone MCP server for testing the Streamable HTTP handshake.
//!
//! Run: cargo run --example mcp_server
//! Then: curl -X POST http://127.0.0.1:8080/mcp -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" -d '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0.0"}},"id":1}'

use webfang::config::Config;
use webfang::di::Container;
use webfang::infrastructure::mcp_server::{server::build_mcp_router, McpState};
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = Config::default();
    let container = Container::new(config)
        .await
        .expect("failed to create container");
    let state = McpState::new(container);
    let app = build_mcp_router(state);

    let addr: SocketAddr = "127.0.0.1:8080".parse().expect("invalid address");
    println!("MCP server listening on http://{}/mcp", addr);
    println!("\nTest with:");
    println!(r#"curl -s -X POST http://127.0.0.1:8080/mcp \\"#);
    println!(r#"  -H "Content-Type: application/json" \\"#);
    println!(r#"  -H "Accept: application/json, text/event-stream" \\"#);
    println!(
        r#"  -d '{{"jsonrpc":"2.0","method":"initialize","params":{{"protocolVersion":"2024-11-05","capabilities":{{}},"clientInfo":{{"name":"test","version":"1.0.0"}}}},"id":1}}'"#
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind");
    axum::serve(listener, app).await.expect("server error");
}
