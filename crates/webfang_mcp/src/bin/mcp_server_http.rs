//! MCP Server — Streamable HTTP transport (binary entry point).
//!
//! Launches the webfang MCP server over HTTP on `127.0.0.1:8080/mcp` by
//! default, with full clap-based configuration (`--help` for all flags).
//! This replaces the old `examples/mcp_server.rs` example.

use std::net::SocketAddr;
use std::num::{NonZeroU64, NonZeroUsize};
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use webfang_core::domain::ssrf_guard::WEBFANG_MCP_DISABLE_SSRF_ENV;
use webfang_mcp::mcp_server::server::{
    require_auth_for_external_bind, start_mcp_server, ServerOptions, DEFAULT_MAX_SESSIONS,
    DEFAULT_MCP_ADDR, DEFAULT_SESSION_CAP_WINDOW_SECS,
};
use webfang_mcp::mcp_server::{
    build_container, build_mcp_state, default_dom_inspector, spawn_ai_wiring, McpState,
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

    /// Admission cap: how many new MCP sessions may be created per window
    /// before the server sheds further ones with 429 (#1611, F6). Must be > 0.
    #[arg(
        long,
        env = "WEBFANG_MCP_MAX_SESSIONS",
        default_value_t = DEFAULT_MAX_SESSIONS
    )]
    max_sessions: usize,

    /// Admission cap: how long (seconds) one admitted session keeps its slot in
    /// the budget. Must be > 0. Defaults to rmcp's own session keep-alive.
    #[arg(
        long,
        env = "WEBFANG_MCP_SESSION_CAP_WINDOW_SECS",
        default_value_t = DEFAULT_SESSION_CAP_WINDOW_SECS
    )]
    session_cap_window_secs: u64,

    /// Auth token; if set, requires `Authorization: Bearer <token>`.
    #[arg(long, env = "WEBFANG_MCP_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Enable AI semantic cleaning (requires the `ai` feature at build time).
    #[arg(long, env = "WEBFANG_MCP_AI")]
    enable_ai: bool,

    /// Allowed root directories for absolute `output_dir` and `checkpoint_dir`
    /// paths (#696, #1588). Repeatable or comma-separated. When omitted,
    /// absolute `output_dir`/`checkpoint_dir` values are rejected
    /// (fail-closed); relative paths always work.
    #[arg(long, env = "WEBFANG_MCP_EXPORT_ROOTS", value_delimiter = ',')]
    export_roots: Vec<std::path::PathBuf>,
}

/// Compose the [`McpState`] this binary ships (#1294 NS-01).
///
/// Thin transport-local wrapper over the shared [`build_mcp_state`] root (#1300);
/// see the stdio binary's copy for why the wrapper stays per-binary.
///
/// # Errors
/// Propagates [`build_mcp_state`] failures (`ScraperError::Config`).
fn build_state(
    container: Arc<webfang_core::application::container::Container>,
    export_roots: Vec<std::path::PathBuf>,
) -> webfang_core::error::Result<McpState> {
    Ok(build_mcp_state(container, export_roots)?.with_inspector(default_dom_inspector()))
}

/// Validate the session admission cap (#1611, F6) at the argv boundary.
///
/// `ServerOptions` types both knobs as `NonZero*`, so a zero coming from a flag
/// or `WEBFANG_MCP_MAX_SESSIONS` is a misconfiguration and NOT a mode: clamping
/// it to 1 in silence is how `CategoryLimits` used to hide one, and reading it
/// as "never release" would turn the cap into a lifetime lockout. Both fail
/// fast with a Spanish operator message instead.
fn require_positive_max_sessions(raw: usize) -> Result<NonZeroUsize> {
    NonZeroUsize::new(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "El límite de sesiones debe ser mayor que 0 (recibido: {raw}). Defina --max-sessions o WEBFANG_MCP_MAX_SESSIONS con un valor positivo."
        )
    })
}

/// Window counterpart of [`require_positive_max_sessions`]: zero would mean
/// "a slot is never released", silently converting the cap into a permanent one
/// for the life of the process.
fn require_positive_session_cap_window(raw: u64) -> Result<NonZeroU64> {
    NonZeroU64::new(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "La ventana del límite de sesiones debe ser mayor que 0 segundos (recibido: {raw}). Defina --session-cap-window-secs o WEBFANG_MCP_SESSION_CAP_WINDOW_SECS con un valor positivo."
        )
    })
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
    // Same fail-fast discipline for the session admission cap (REQ-06's
    // rationale, one knob over): a zero cap is rejected here, before the
    // container/downloader exist, not at the composition below.
    let max_sessions = require_positive_max_sessions(args.max_sessions)?;
    let session_cap_window_secs =
        require_positive_session_cap_window(args.session_cap_window_secs)?;
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

    // Shared composition root (#1300) + the DOM inspector this binary ships
    // (#1294 NS-01). The bounded shared Downloader comes from `build_mcp_state`
    // — the same budget-derived cache policy as the CLI, never the legacy
    // unbounded `Downloader::new` path (#1120).
    let state = build_state(container, args.export_roots)?;

    let opts = ServerOptions {
        request_timeout_secs: args.timeout_secs,
        body_limit_bytes: args.body_limit,
        rate_per_second: args.rate,
        rate_burst: args.burst,
        auth_token: args.auth_token,
        max_sessions,
        session_cap_window_secs,
    };

    // Disable SSRF for testing with env var, otherwise use default (enabled)
    if std::env::var(WEBFANG_MCP_DISABLE_SSRF_ENV).is_ok() {
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
        let state = build_state(container, Vec::new()).expect("HTTP state composes");
        assert!(
            state.inspector.is_some(),
            "the HTTP server must wire a DOM inspector; a `None` here silences every \
                 selector diagnostic an MCP client asks for"
        );
    }

    /// Same contract as the stdio transport: `--export-roots` must survive the
    /// composition root (#696 fail-closed allowlist). Pinned on both because both
    /// now build their state through a helper that could drop an argument.
    #[tokio::test]
    async fn http_composition_root_keeps_the_export_roots_contract() {
        let config = webfang_core::config::Config::default();
        let container = Arc::new(
            webfang_core::di::Container::new(config.crawler, config.scraper)
                .await
                .expect("container creation failed"),
        );
        let roots = vec![std::path::PathBuf::from("/srv/allowed")];

        let state = build_state(container, roots.clone()).expect("HTTP state composes");
        assert_eq!(
            state.allowed_export_roots.as_slice(),
            roots.as_slice(),
            "HTTP must honor --export-roots / WEBFANG_MCP_EXPORT_ROOTS (#696)"
        );
    }

    /// #1611 F6: a zero cap fails fast at the argv boundary with a Spanish
    /// message naming both configuration surfaces. The old `CategoryLimits`
    /// clamp made a zero limit silently mean 1 and hid the misconfiguration —
    /// pinning the refusal is what keeps that from coming back here.
    #[test]
    fn zero_session_cap_is_refused_with_a_spanish_message() {
        let err = require_positive_max_sessions(0)
            .expect_err("a cap of zero must be refused, not clamped")
            .to_string();
        assert!(
            err.contains("--max-sessions"),
            "message must name the flag: {err}"
        );
        assert!(
            err.contains("WEBFANG_MCP_MAX_SESSIONS"),
            "message must name the env var: {err}"
        );
        assert_eq!(
            require_positive_max_sessions(1)
                .expect("one is valid")
                .get(),
            1
        );
        assert_eq!(
            require_positive_max_sessions(64).expect("valid cap").get(),
            64
        );
    }

    /// Same for the window: zero would mean "never release a slot", i.e. a
    /// lifetime cap nobody asked for.
    #[test]
    fn zero_session_cap_window_is_refused_with_a_spanish_message() {
        let err = require_positive_session_cap_window(0)
            .expect_err("a zero window must be refused, not clamped")
            .to_string();
        assert!(
            err.contains("--session-cap-window-secs"),
            "message must name the flag: {err}"
        );
        assert!(
            err.contains("WEBFANG_MCP_SESSION_CAP_WINDOW_SECS"),
            "message must name the env var: {err}"
        );
        assert_eq!(
            require_positive_session_cap_window(1)
                .expect("one second is valid")
                .get(),
            1
        );
    }
}
