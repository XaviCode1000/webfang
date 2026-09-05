//! MCP Server — Stdio transport (binary entry point).
//!
//! Launches the webfang MCP server over stdin/stdout for clients that spawn
//! the server as a subprocess (OpenCode, Claude Desktop, Cline, etc.). This
//! replaces the old `examples/mcp_server_stdio.rs` example.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};

use clap::Parser;
use rmcp::service::ServiceExt;
use tokio::io::AsyncWrite;
use tokio::sync::Notify;
use webfang_core::cli::error::{CliExit, EXIT_IO_ERROR};
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

// #1151: shared death signal for the stdout half of the stdio transport.
//
// rmcp's server loop discards handler-response send errors and only quits on
// stdin EOF or cancellation — so after a successful handshake, a client that
// closes its read end of stdout leaves `server.waiting()` pending forever
// with no exit code and no log. The Rust runtime ignores SIGPIPE, hence the
// broken pipe surfaces as an `Err` from `AsyncWrite`, not a signal.
// Recording the first write failure here lets `main()` observe the transport
// death at our layer and shut down cleanly. No wall-clock timeout around
// `waiting()`: MCP sessions are legitimately long-lived and a timeout would
// kill healthy ones.
#[derive(Debug, Clone)]
struct StdoutDeathSignal {
    inner: Arc<SignalInner>,
}

#[derive(Debug)]
struct SignalInner {
    broken: AtomicBool,
    notify: Notify,
    first_error: std::sync::Mutex<Option<String>>,
}

impl StdoutDeathSignal {
    fn new() -> Self {
        Self {
            inner: Arc::new(SignalInner {
                broken: AtomicBool::new(false),
                notify: Notify::new(),
                first_error: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Record a stdout write failure, keeping only the first message for the
    /// shutdown log. Never blocks: the mutex is held for a single `Option`
    /// store, never across `.await` (this runs inside `poll_write`).
    fn mark_broken(&self, error: &std::io::Error) {
        if !self.inner.broken.swap(true, Ordering::SeqCst) {
            if let Ok(mut slot) = self.inner.first_error.lock() {
                *slot = Some(error.to_string());
            }
            self.inner.notify.notify_waiters();
        }
    }

    fn first_error_message(&self) -> Option<String> {
        self.inner
            .first_error
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// Resolve when the stdout half dies. The notified future is created
    /// BEFORE checking the flag so a `mark_broken` racing this check cannot
    /// be missed (no wall-clock timeout involved).
    #[tracing::instrument(skip(self), name = "mcp_stdio_stdout_death_watch")]
    async fn wait_broken(&self) {
        loop {
            let notified = self.inner.notify.notified();
            if self.inner.broken.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

/// `AsyncWrite` adapter that observes stdout transport death at our layer
/// (#1151). Forwards every call to the inner writer unchanged; on failure it
/// raises the shared [`StdoutDeathSignal`] and returns the error to rmcp
/// untouched, so the wire behavior is identical and only the observability
/// is new.
#[derive(Debug)]
struct ObservingStdout<W> {
    inner: W,
    signal: StdoutDeathSignal,
}

impl<W> AsyncWrite for ObservingStdout<W>
where
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(Err(error)) => {
                self.signal.mark_broken(&error);
                Poll::Ready(Err(error))
            },
            other => other,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_flush(cx) {
            Poll::Ready(Err(error)) => {
                self.signal.mark_broken(&error);
                Poll::Ready(Err(error))
            },
            other => other,
        }
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match Pin::new(&mut self.inner).poll_shutdown(cx) {
            Poll::Ready(Err(error)) => {
                self.signal.mark_broken(&error);
                Poll::Ready(Err(error))
            },
            other => other,
        }
    }
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
    // #1151: main owns the AsyncWrite handed to serve(). Wrapping stdout
    // records post-handshake write failures (EPIPE when the client closes
    // its read end) that rmcp would otherwise swallow while `waiting()`
    // pends forever.
    let stdout_signal = StdoutDeathSignal::new();
    let stdout = ObservingStdout {
        inner: tokio::io::stdout(),
        signal: stdout_signal.clone(),
    };
    let transport = (tokio::io::stdin(), stdout);
    let server = match handler.serve(transport).await {
        Ok(server) => server,
        Err(e) => {
            tracing::error!(error = %e, "mcp stdio serve failed");
            return CliExit::IoError(format!("No se pudo iniciar el servidor MCP por stdio: {e}"));
        },
    };

    // Wait for the server to finish (client disconnects or stdin closes)
    // — or for OUR layer to observe the stdout half dying underneath a
    // live session. No wall-clock timeout: MCP sessions are legitimately
    // long-lived and a timeout would kill healthy ones (#1151).
    let session_outcome = tokio::select! {
        result = server.waiting() => Some(result),
        () = stdout_signal.wait_broken() => None,
    };

    // #1121: same drain as the HTTP transport (server.rs) — the stdio tools
    // persist crawl results through the very same background writer, so on
    // this transport too `shutdown()` must join it before the runtime goes
    // away. Runs even if `waiting()` errored; its error is propagated after.
    if let Some(repo) = exit_container.crawl_result_repository() {
        if let Err(e) = repo.shutdown().await {
            tracing::warn!(error = %e, "crawl-result writer shutdown reported errors");
        }
    }

    // A post-handshake stdout death (client closed the read end) never
    // surfaces through `waiting()` — rmcp swallows the write error — so
    // it gets the same clean log + I/O-error exit instead of hanging
    // forever with no exit code and no log (#1151).
    let Some(waiting_result) = session_outcome else {
        let detail = stdout_signal
            .first_error_message()
            .unwrap_or_else(|| "el cliente cerró la tubería de salida".to_string());
        tracing::error!(error = %detail, "mcp stdio stdout broken pipe: client closed read end, shutting down");
        let message = format!("El servidor MCP por stdio terminó con error: {detail}");
        // Mirror of `CliExit::IoError`'s `Termination::report` (which is
        // bypassed below): user-facing Spanish line on stderr + EX_IOERR.
        eprintln!("Error: {message}");
        // `std::process::exit`, not `return`: the dead client still holds
        // the stdin write end open, so the blocking-pool thread parked in
        // `read(stdin)` by the abandoned serve loop can never observe EOF
        // — and `Runtime` drop joins that thread, which would hang this
        // branch exactly like the original bug (verified via gdb: main in
        // `BlockingPool` drop, worker in `read(stdin)`). The crawl-result
        // writer above is already drained; the OS reaps the rest.
        std::process::exit(EXIT_IO_ERROR.into());
    };

    // Normal EOF shutdown returns Ok → exit 0; a transport error mid-session
    // gets the same clean log + exit treatment instead of a panic backtrace
    // aimed at the spawning MCP client (#1108).
    if let Err(e) = waiting_result {
        tracing::error!(error = %e, "mcp server terminated with error");
        return CliExit::IoError(format!("El servidor MCP por stdio terminó con error: {e}"));
    }

    CliExit::Success
}
