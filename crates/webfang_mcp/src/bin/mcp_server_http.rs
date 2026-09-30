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
    require_auth_or_explicit_anonymous, start_mcp_server, ServerOptions, DEFAULT_MAX_SESSIONS,
    DEFAULT_MCP_ADDR, DEFAULT_SESSION_CAP_WINDOW_SECS, MAX_ALLOWED_SESSIONS_CAP,
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
    /// before the server sheds further ones with 429 (#1611, F6). Must be
    /// > 0 and <= MAX_ALLOWED_SESSIONS_CAP.
    #[arg(
        long,
        env = "WEBFANG_MCP_MAX_SESSIONS",
        default_value_t = DEFAULT_MAX_SESSIONS
    )]
    max_sessions: usize,

    /// Admission cap: how long (seconds) one admitted session keeps its slot in
    /// the budget. Must be > 0. Defaults to rmcp's own session keep-alive.
    /// Shorter values are accepted with a warning: the cap then bounds the
    /// CREATION RATE, not the number of live sessions.
    #[arg(
        long,
        env = "WEBFANG_MCP_SESSION_CAP_WINDOW_SECS",
        default_value_t = DEFAULT_SESSION_CAP_WINDOW_SECS
    )]
    session_cap_window_secs: u64,

    /// Auth token; if set, requires `Authorization: Bearer <token>`.
    #[arg(long, env = "WEBFANG_MCP_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Serve requests with NO token configured (#1611, G-18) — the development
    /// mode. Off by default: an unset credential used to mean "anyone who can
    /// reach the socket", which is a fail-open default for a security boundary.
    /// Only honoured on a loopback bind; a routable bind without a token is
    /// still refused.
    #[arg(long, env = "WEBFANG_MCP_ALLOW_ANONYMOUS")]
    allow_anonymous: bool,

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
///
/// The upper bound is the same discipline applied to MAGNITUDE rather than to
/// zero: `SessionCap` pre-reserves its deque to the accepted maximum, so an
/// absurd value is not a big number, it is a multi-gigabyte reservation that
/// aborts the process with a raw allocator message before this function can
/// say anything. See [`MAX_ALLOWED_SESSIONS_CAP`] for the ceiling and its
/// justification.
fn require_positive_max_sessions(raw: usize) -> Result<NonZeroUsize> {
    if raw > MAX_ALLOWED_SESSIONS_CAP {
        return Err(anyhow::anyhow!(
            "El límite de sesiones no puede superar {MAX_ALLOWED_SESSIONS_CAP} (recibido: {raw}). Defina --max-sessions o WEBFANG_MCP_MAX_SESSIONS con un valor dentro del rango."
        ));
    }
    NonZeroUsize::new(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "El límite de sesiones debe ser mayor que 0 (recibido: {raw}). Defina --max-sessions o WEBFANG_MCP_MAX_SESSIONS con un valor positivo."
        )
    })
}

/// Spanish warning for a window shorter than rmcp's session keep-alive: a slot
/// is released one window after its admission, so a window shorter than the
/// session's maximum life no longer bounds the number of LIVE sessions — it
/// bounds the creation rate instead, while the session map keeps growing until
/// rmcp reaps it. Accepted with a warning rather than refused, because a short
/// window is a legitimate (if different) policy, and silently accepting it
/// leaves the operator believing a cap is on when it is not bounding anything
/// that matters.
const SHORT_WINDOW_WARNING: &str =
    "La ventana del límite de sesiones es más corta que el keep-alive de sesión de rmcp: el límite ya no acota las sesiones vivas, solo la tasa de creación. Aumente --session-cap-window-secs o WEBFANG_MCP_SESSION_CAP_WINDOW_SECS.";

/// Window counterpart of [`require_positive_max_sessions`]: zero would mean
/// "a slot is never released", silently converting the cap into a permanent one
/// for the life of the process. A window that is merely SHORT is accepted, with
/// the operator told what it changes (see [`SHORT_WINDOW_WARNING`]).
fn require_positive_session_cap_window(raw: u64) -> Result<NonZeroU64> {
    let window = NonZeroU64::new(raw).ok_or_else(|| {
        anyhow::anyhow!(
            "La ventana del límite de sesiones debe ser mayor que 0 segundos (recibido: {raw}). Defina --session-cap-window-secs o WEBFANG_MCP_SESSION_CAP_WINDOW_SECS con un valor positivo."
        )
    })?;

    if raw < DEFAULT_SESSION_CAP_WINDOW_SECS {
        tracing::warn!(
            session_cap_window_secs = raw,
            keep_alive_secs = DEFAULT_SESSION_CAP_WINDOW_SECS,
            user_message = SHORT_WINDOW_WARNING,
            "session admission cap window is shorter than the session keep-alive — \
             the cap bounds the creation rate, not the number of live sessions"
        );
    }

    Ok(window)
}

/// Spanish warning printed when the operator explicitly asks for anonymous
/// operation (#1611, G-18): the mode is legitimate, but it should never be
/// mistaken for a hardened deployment.
const ANONYMOUS_START_WARNING: &str =
    "El servidor MCP acepta peticiones sin token en loopback. Cualquier proceso local puede usar todas las herramientas. Defina --auth-token (o WEBFANG_MCP_AUTH_TOKEN) en cualquier despliegue compartido.";

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
    // container/downloader. #1611 G-18 adds the second refusal: with the
    // fail-closed default, "no token AND no opt-in" is a misconfiguration too,
    // and a server that starts and then 401s everything looks like a broken
    // deployment.
    require_auth_or_explicit_anonymous(args.bind, args.auth_token.is_some(), args.allow_anonymous)?;
    // Same fail-fast discipline for the session admission cap (REQ-06's
    // rationale, one knob over): a zero cap is rejected here, before the
    // container/downloader exist, not at the composition below.
    let max_sessions = require_positive_max_sessions(args.max_sessions)?;
    let session_cap_window_secs =
        require_positive_session_cap_window(args.session_cap_window_secs)?;
    if args.bind.ip().is_loopback() && args.auth_token.is_none() {
        tracing::warn!(
            user_message = ANONYMOUS_START_WARNING,
            "MCP server starting on loopback with anonymous access explicitly allowed \
             (development mode) — every local process may call every tool"
        );
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
        allow_anonymous: args.allow_anonymous,
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

    /// A magnitude is a misconfiguration too, and a much more expensive one:
    /// the cap pre-reserves its deque to the accepted maximum, so
    /// `--max-sessions 1000000000` is a ~16 GB `Instant` reservation that
    /// aborts the process with a raw allocator message — the exact failure the
    /// fail-fast discipline above exists to prevent, arriving one step later.
    #[test]
    fn an_absurd_session_cap_is_refused_with_a_spanish_message() {
        let err = require_positive_max_sessions(MAX_ALLOWED_SESSIONS_CAP + 1)
            .expect_err("a cap above the ceiling must be refused, not accepted")
            .to_string();
        assert!(
            err.contains("--max-sessions"),
            "message must name the flag: {err}"
        );
        assert!(
            err.contains("WEBFANG_MCP_MAX_SESSIONS"),
            "message must name the env var: {err}"
        );
        assert!(
            err.contains(&MAX_ALLOWED_SESSIONS_CAP.to_string()),
            "message must name the accepted ceiling: {err}"
        );
        // The ceiling itself is valid — the bound must not drift into rejecting
        // the largest value the crate documents as accepted.
        assert_eq!(
            require_positive_max_sessions(MAX_ALLOWED_SESSIONS_CAP)
                .expect("the ceiling is accepted")
                .get(),
            MAX_ALLOWED_SESSIONS_CAP
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

    /// Run `f` with an in-memory `tracing` subscriber and return what it logged.
    ///
    /// The warning is an operator-facing event, not a return value, so a test
    /// that only checks the returned `NonZeroU64` would pass with the warning
    /// deleted — which is the whole behavior here.
    fn capture_logs(f: impl FnOnce()) -> String {
        use std::sync::{Arc, Mutex};

        /// Shared sink behind the subscriber's writer.
        #[derive(Clone, Default)]
        struct Buffer(Arc<Mutex<Vec<u8>>>);

        impl std::io::Write for Buffer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .expect("capture buffer lock is not poisoned")
                    .write(bytes)
            }

            fn flush(&mut self) -> std::io::Result<()> {
                self.0
                    .lock()
                    .expect("capture buffer lock is not poisoned")
                    .flush()
            }
        }

        let buffer = Buffer::default();
        let sink = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || sink.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buffer
            .0
            .lock()
            .expect("capture buffer lock is not poisoned")
            .clone();
        String::from_utf8(bytes).expect("tracing writes UTF-8")
    }

    /// A window shorter than rmcp's keep-alive is accepted, but the operator
    /// is told: from then on the cap bounds the creation RATE, not the number
    /// of live sessions, and a server that looks capped is not.
    #[test]
    fn a_short_window_warns_in_spanish_that_live_sessions_are_unbounded() {
        let logs = capture_logs(|| {
            require_positive_session_cap_window(1).expect("one second is still valid");
        });
        assert!(
            logs.contains("sesiones vivas"),
            "the warning must say what stopped being bounded: {logs}"
        );
        assert!(
            logs.contains(&DEFAULT_SESSION_CAP_WINDOW_SECS.to_string()),
            "the warning must name the window that is actually needed: {logs}"
        );
        assert!(
            logs.contains("keep-alive"),
            "the warning must name rmcp's keep-alive, the reason for the number: {logs}"
        );
    }

    /// And the warning does not fire for a window that does bound live
    /// sessions — including the default every deployment starts from.
    #[test]
    fn a_window_at_or_above_keep_alive_does_not_warn() {
        for window in [
            DEFAULT_SESSION_CAP_WINDOW_SECS,
            DEFAULT_SESSION_CAP_WINDOW_SECS + 60,
        ] {
            let logs = capture_logs(move || {
                require_positive_session_cap_window(window).expect("valid window");
            });
            assert!(
                !logs.contains("keep-alive"),
                "a {window}s window still bounds live sessions, so it must not warn: {logs}"
            );
        }
    }
}
