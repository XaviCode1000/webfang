//! MCP Server — Streamable HTTP transport (binary entry point).
//!
//! Launches the webfang MCP server over HTTP on `127.0.0.1:8080/mcp` by
//! default, with full clap-based configuration (`--help` for all flags).
//! This replaces the old `examples/mcp_server.rs` example.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use webfang_mcp::mcp_server::server::{
    require_auth_for_external_bind, start_mcp_server, ServerOptions, DEFAULT_MCP_ADDR,
};
use webfang_mcp::mcp_server::{
    build_container, build_shared_downloader, default_dom_inspector, spawn_ai_wiring, McpState,
};

/// Webfang MCP Server — Streamable HTTP transport.
#[derive(Parser, Debug)]
#[command(
    name = "webfang-mcp",
    version,
    about = "Webfang MCP Server (HTTP transport)",
    long_about = "Exposes 36 tools via the Model Context Protocol over Streamable HTTP."
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

/// Compose the [`McpState`] this binary ships (#1294 NS-01).
///
/// Split out of `main` because `main` is unreachable from a test and the
/// composition root is the exact place the DOM inspector enters the MCP process:
/// `McpState::inspector` defaults to `None`, so a server that never wires one
/// answers every selector failure without diagnostics.
fn build_state(
    container: Arc<webfang_core::di::Container>,
    downloader: Arc<webfang_core::adapters::downloader::Downloader>,
    export_roots: Vec<std::path::PathBuf>,
) -> McpState {
    McpState::from_container(container)
        .with_downloader(downloader)
        .with_inspector(default_dom_inspector())
        .with_export_roots(export_roots)
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

    // Build the container FAST — no model resolution happens here (#759).
    // The AI ports are wired lazily in a background task after the container
    // is shared with the server state. A construction failure propagates as a
    // typed error to the supervisor: stderr message + exit code, never a
    // panic backtrace (#1123).
    let container =
        Arc::new(build_container().await.map_err(|e| {
            anyhow::anyhow!("No se pudo crear el contenedor del servidor MCP: {e}")
        })?);

    if args.enable_ai {
        spawn_ai_wiring(Arc::clone(&container));
    }

    // Inject a shared Downloader so `download_assets` reuses one connection pool
    // across tool calls. The default config writes to `./downloads` relative to
    // the working directory. #1120: built through the bounded composition-root
    // helper — the same budget-derived cache policy as the CLI — never the legacy
    // unbounded `Downloader::new` path.
    let state = build_state(
        container,
        Arc::new(build_shared_downloader()?),
        args.export_roots,
    );

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

#[cfg(test)]
mod tests {
    use super::*;

    /// #1294 NS-01: this transport's composition root must ship an inspector.
    ///
    /// The claim is narrow on purpose — wiring, not behavior: what the helper
    /// returns is pinned next to it in `mcp_server/mod.rs`. Nothing in the tree
    /// asserted this before, which is why the field could stay `None` in every
    /// production MCP process while its unit tests kept passing.
    #[tokio::test]
    async fn http_composition_root_wires_an_inspector() {
        let config = webfang_core::config::Config::default();
        let container = Arc::new(
            webfang_core::di::Container::new(config.crawler, config.scraper)
                .await
                .expect("container creation failed"),
        );
        let downloader =
            Arc::new(build_shared_downloader().expect("bounded shared downloader builds"));

        let state = build_state(container, downloader, Vec::new());
        assert!(
            state.inspector.is_some(),
            "the HTTP server must wire a DOM inspector; a `None` here silences every \
             selector diagnostic an MCP client asks for"
        );
    }
}
