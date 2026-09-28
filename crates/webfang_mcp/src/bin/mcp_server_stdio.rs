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
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Notify;
use webfang_core::cli::error::{CliExit, EXIT_IO_ERROR};
use webfang_mcp::mcp_server::{
    build_container, build_mcp_state, default_dom_inspector, panic_hook::setup_panic_hook,
    spawn_ai_wiring, McpHandler, McpState,
};

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

    /// Allowed root directories for absolute `output_dir` and `checkpoint_dir`
    /// paths (#696, #1588). Repeatable or comma-separated. When omitted,
    /// absolute `output_dir`/`checkpoint_dir` values are rejected
    /// (fail-closed); relative paths always work.
    #[arg(long, env = "WEBFANG_MCP_EXPORT_ROOTS", value_delimiter = ',')]
    export_roots: Vec<std::path::PathBuf>,
}

// #1151: shared death signal for EITHER half of the stdio transport.
//
// rmcp's server loop discards handler-response send errors and only quits on
// stdin EOF or cancellation — so after a successful handshake, a client that
// closes its read end of stdout leaves `server.waiting()` pending forever
// with no exit code and no log. The Rust runtime ignores SIGPIPE, hence the
// broken pipe surfaces as an `Err` from `AsyncWrite`, not a signal.
// Recording the first transport failure here lets `main()` observe the death
// at our layer and shut down cleanly. No wall-clock timeout around
// `waiting()`: MCP sessions are legitimately long-lived and a timeout would
// kill healthy ones.
//
// #1611 F7 widened "either half": the stdin wrapper added below records the
// read half's rejection (an input frame past [`MAX_STDIN_LINE_BYTES`]) through
// the same signal, because rmcp reports that one exactly as it reports a dead
// pipe — `receive()` logs and returns `None`, which is indistinguishable from a
// client hangup — and an admission-control refusal must NOT look like an
// ordinary disconnect.
#[derive(Debug, Clone)]
struct TransportDeathSignal {
    inner: Arc<SignalInner>,
}

#[derive(Debug)]
struct SignalInner {
    broken: AtomicBool,
    notify: Notify,
    first_error: std::sync::Mutex<Option<String>>,
}

impl TransportDeathSignal {
    fn new() -> Self {
        Self {
            inner: Arc::new(SignalInner {
                broken: AtomicBool::new(false),
                notify: Notify::new(),
                first_error: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Record a transport failure, keeping only the first message for the
    /// shutdown log. Never blocks: the mutex is held for a single `Option`
    /// store, never across `.await` (this runs inside `poll_write` /
    /// `poll_read`).
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

    /// Resolve when either half dies. The notified future is created
    /// BEFORE checking the flag so a `mark_broken` racing this check cannot
    /// be missed (no wall-clock timeout involved).
    #[tracing::instrument(skip(self), name = "mcp_stdio_transport_death_watch")]
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
/// raises the shared [`TransportDeathSignal`] and returns the error to rmcp
/// untouched, so the wire behavior is identical and only the observability
/// is new.
#[derive(Debug)]
struct ObservingStdout<W> {
    inner: W,
    signal: TransportDeathSignal,
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

/// Max bytes accepted for ONE newline-delimited JSON-RPC frame on stdin
/// (#1611 F7).
///
/// rmcp's stdio transport reads with `read_until(b'\n', &mut line_buf)` over an
/// UNBOUNDED `Vec<u8>` (`rmcp-1.8.0/src/transport/async_rw.rs:125-133`, and
/// the `line_buf` field at `:52`), so a peer that never sends a newline picks
/// how much memory this process allocates before a single byte is parsed. The
/// `JsonRpcMessageCodec::new_with_max_length` knob that exists in the same file
/// (`async_rw.rs:196`) cannot bound it: that is a `Decoder` setting, and the
/// server's read path builds its codec with `default()` (`async_rw.rs:60-64`) —
/// it is unreachable from here, so a read wrapper is the only place the bound
/// can live.
///
/// 1 MiB is this crate's own "largest legitimate input" figure
/// (`webfang_mcp::mcp_server::validation::MAX_BLOB_LEN`) AND the frame budget
/// the `urls` cap needs: the biggest legitimate `scrape_batch` request is
/// [`URLS_MAX`](webfang_mcp::mcp_server::params::URLS_MAX) × `MAX_URL_LEN`
/// = 100 × 8 KiB ≈ 800 KiB, so no call the validator accepts is refused here.
/// The two caps are one budget seen from both ends — a cap that could reject a
/// valid batch would be a bug, not a defense.
pub const MAX_STDIN_LINE_BYTES: usize = 1_048_576;

/// `AsyncRead` adapter that caps one input frame at [`MAX_STDIN_LINE_BYTES`]
/// (#1611 F7) — the read-side analogue of [`ObservingStdout`], for the same
/// reason: rmcp swallows the transport failure, so without an adapter at our
/// layer the refusal is invisible and indistinguishable from a hangup.
///
/// Bytes are forwarded to rmcp untouched; the count is PER FRAME, reset at
/// every `\n` (JSON-RPC over stdio is newline-delimited), so a long but legal
/// session is never throttled by its own history. Crossing the cap raises the
/// shared [`TransportDeathSignal`], logs a structured record, and fails the
/// read with `InvalidData` (on the poll AFTER the offending bytes — see
/// [`BoundedStdin::pending_error`]) — which is what makes the refusal
/// observable from `main()`:
///
/// - rmcp's `receive()` answers a read error with a `tracing::error!` and
///   `None` (`async_rw.rs:131-135`), i.e. the very same "clean EOF" a client
///   hangup produces, so a silent exit 0 is the alternative this replaces;
/// - a JSON-RPC `-32700` parse error was considered and rejected: this is a
///   transport-level admission refusal, not a malformed frame — a peer that
///   sends garbage still gets rmcp's parse error, and one that sends too much
///   gets a logged, non-zero exit the operator can see.
#[derive(Debug)]
struct BoundedStdin<R> {
    inner: R,
    signal: TransportDeathSignal,
    max_line_bytes: usize,
    line_bytes: usize,
    /// Refusal latched until the caller observes it.
    ///
    /// It cannot be returned on the same poll that delivered the bytes which
    /// crossed the cap: `AsyncRead` consumers treat a read that fills its
    /// buffer AND returns `Err` as a contract violation — tokio's
    /// `read_to_end` debug-asserts that a failing read read nothing
    /// (`io/util/read_to_end.rs:125`), and a debug build panics on it. So the
    /// offending bytes are handed over first and the error lands on the next,
    /// empty poll — which is also what drops the rest of the garbage line
    /// instead of buffering it.
    pending_error: Option<std::io::Error>,
}

impl<R> BoundedStdin<R> {
    fn new(inner: R, signal: TransportDeathSignal, max_line_bytes: usize) -> Self {
        Self {
            inner,
            signal,
            max_line_bytes,
            line_bytes: 0,
            pending_error: None,
        }
    }
}

impl<R> AsyncRead for BoundedStdin<R>
where
    R: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if let Some(error) = self.pending_error.take() {
            return Poll::Ready(Err(error));
        }
        let before = buf.filled().len();
        let (completed, remaining) = match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let fresh = &buf.filled()[before..];
                match fresh.iter().position(|byte| *byte == b'\n') {
                    // The frame that just ended is the one rmcp is about to
                    // parse: `line_bytes + nl` bytes, newline excluded. What
                    // follows it starts a NEW frame, counted from zero.
                    Some(nl) => (self.line_bytes.saturating_add(nl), fresh.len() - nl - 1),
                    // No terminator in this chunk: the open frame just grew.
                    None => (0, self.line_bytes.saturating_add(fresh.len())),
                }
            },
            other => return other,
        };
        self.line_bytes = remaining;

        // One chunk can touch two frames, so the refusal measures the LARGER
        // of them: the one that just completed and the one still open.
        // Reporting `self.line_bytes` alone would print the empty remainder
        // after a completed oversize frame — a cap trip logged as "0 bytes".
        let refused_at = completed.max(self.line_bytes);
        if refused_at > self.max_line_bytes {
            let message = format!(
                "JSON-RPC frame exceeds the {}-byte stdin cap (refused at {refused_at} bytes)",
                self.max_line_bytes
            );
            // Structured at the point of refusal, where the numbers are known;
            // `main()` adds the shutdown line when it acts on the signal.
            tracing::error!(
                limit_bytes = self.max_line_bytes,
                refused_at_bytes = refused_at,
                "mcp stdio stdin refused: input frame exceeded the per-line cap",
            );
            self.signal.mark_broken(&std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message.clone(),
            ));
            // This poll already delivered bytes, so the refusal waits for the
            // next one (see `pending_error`).
            self.pending_error = Some(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                message,
            ));
        }
        Poll::Ready(Ok(()))
    }
}

/// Report a dead stdio transport and leave the process with the I/O error code.
///
/// `std::process::exit`, not `return`: the peer that broke the transport may
/// still hold the OTHER pipe open, so the blocking-pool thread parked in
/// `read(stdin)` by the abandoned serve loop can never observe EOF — and
/// `Runtime` drop joins that thread, which would hang the shutdown path
/// exactly like the bug it replaced (verified via gdb: main in `BlockingPool`
/// drop, worker in `read(stdin)`). The crawl-result writer is already drained
/// by the caller; the OS reaps the rest.
///
/// The user-facing line is Spanish on stderr, matching `CliExit::IoError`'s
/// `Termination::report` that this bypasses; stdout stays reserved for JSON-RPC.
fn exit_transport_death(event: &'static str, detail: &str) -> ! {
    tracing::error!(error = %detail, "mcp stdio transport dead: {event}, shutting down");
    let message = format!("El servidor MCP por stdio terminó con error: {detail}");
    eprintln!("Error: {message}");
    std::process::exit(EXIT_IO_ERROR.into());
}

/// Compose the [`McpState`] this binary ships (#1294 NS-01).
///
/// Thin transport-local wrapper over the shared [`build_mcp_state`] root (#1300):
/// the bounded shared downloader and the export roots come from there, and the
/// DOM inspector is what #1294 adds on top — `McpState::inspector` defaults to
/// `None`, so a server that never wires one answers every selector failure
/// without diagnostics. Kept per-binary on purpose: a transport that stops
/// composing through its wrapper trips `dead_code`, which the shared root alone
/// would hide (#1305's own test rationale).
///
/// # Errors
/// Propagates [`build_mcp_state`] failures (`ScraperError::Config`).
fn build_state(
    container: Arc<webfang_core::application::container::Container>,
    export_roots: Vec<std::path::PathBuf>,
) -> webfang_core::error::Result<McpState> {
    Ok(build_mcp_state(container, export_roots)?.with_inspector(default_dom_inspector()))
}

#[tokio::main]
async fn main() -> CliExit {
    // All logging to stderr — stdout is reserved for JSON-RPC.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();

    // #1626 PC-3: transport parity. `start_mcp_server` (server.rs) installs
    // this hook, and a CONTAINED panic on stdio owes the operator the same
    // structured record — message AND location — that the HTTP transport
    // guarantees. Without it, stdio logs the payload from `call_tool` but the
    // panic LOCATION is only whatever the default hook prints.
    //
    // Safe to add on this transport for the same reason it is there at all: the
    // hook emits `tracing::error!` through the subscriber installed just above
    // and then delegates to the default hook, and BOTH write to stderr, which
    // this transport reserves for logs (see the serve comment below). stdout
    // carries JSON-RPC and is never touched.
    setup_panic_hook();

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

    // Shared composition root (#1300) + the DOM inspector this binary must ship
    // (#1294 NS-01). `build_state` is deliberately transport-local: if one
    // binary stops composing through it, `dead_code` catches the regression —
    // something a single shared root cannot see from the other binary's test.
    let state = match build_state(container, args.export_roots) {
        Ok(state) => state,
        Err(e) => {
            tracing::error!(error = %e, "MCP stdio boot failed: shared downloader construction");
            return CliExit::ConfigError(format!(
                "No se pudo construir el estado del servidor MCP: {e}"
            ));
        },
    };

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
    // #1611 F7: the same treatment for the read half — `BoundedStdin` is
    // main's AsyncRead, and rmcp's blanket transport impl accepts it as-is
    // (`AsyncRead + Send + 'static + Unpin`, `async_rw.rs:24-31`), so the
    // per-frame cap costs exactly one type change at the call site.
    let stdin_signal = TransportDeathSignal::new();
    let stdin = BoundedStdin::new(
        tokio::io::stdin(),
        stdin_signal.clone(),
        MAX_STDIN_LINE_BYTES,
    );
    let stdout_signal = TransportDeathSignal::new();
    let stdout = ObservingStdout {
        inner: tokio::io::stdout(),
        signal: stdout_signal.clone(),
    };
    let transport = (stdin, stdout);
    let server = match handler.serve(transport).await {
        Ok(server) => server,
        Err(e) => {
            tracing::error!(error = %e, "mcp stdio serve failed");
            return CliExit::IoError(format!("No se pudo iniciar el servidor MCP por stdio: {e}"));
        },
    };

    // Wait for the server to finish (client disconnects or stdin closes)
    // — or for OUR layer to observe EITHER half dying underneath a
    // live session. No wall-clock timeout: MCP sessions are legitimately
    // long-lived and a timeout would kill healthy ones (#1151).
    let session_outcome = tokio::select! {
        result = server.waiting() => Some(result),
        () = stdout_signal.wait_broken() => None,
        () = stdin_signal.wait_broken() => None,
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

    // Neither half surfaces its own death through `waiting()` — rmcp swallows
    // the write error (#1151) and turns a read error into the same `None` a
    // hangup produces (F7) — so both get the same clean log + I/O-error exit
    // instead of hanging forever with no exit code and no log. The stdin half
    // is checked FIRST because a refusal is the more actionable fact when both
    // tripped: the client sent something the server will not accept.
    let Some(waiting_result) = session_outcome else {
        if let Some(detail) = stdin_signal.first_error_message() {
            exit_transport_death("stdin frame refused by the admission cap", &detail);
        }
        let detail = stdout_signal
            .first_error_message()
            .unwrap_or_else(|| "el cliente cerró la tubería de salida".to_string());
        exit_transport_death("stdout broken pipe: client closed read end", &detail);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// #1294 NS-01: the stdio transport is the one most MCP clients actually
    /// spawn, and it shipped the same unwired state as HTTP. Both composition
    /// roots are pinned separately on purpose: sharing one helper would hide a
    /// regression in whichever binary stops using it.
    #[tokio::test]
    async fn stdio_composition_root_wires_an_inspector() {
        let config = webfang_core::config::Config::default();
        let container = Arc::new(
            webfang_core::di::Container::new(config.crawler, config.scraper)
                .await
                .expect("container creation failed"),
        );

        let state = build_state(container, Vec::new()).expect("stdio state composes");
        assert!(
            state.inspector.is_some(),
            "the stdio server must wire a DOM inspector; a `None` here silences \
                 every selector diagnostic an MCP client asks for"
        );
    }

    /// The extraction that lost a parameter is the reason this exists: moving the
    /// chain out of `main` silently dropped `with_export_roots`, which would have
    /// turned #696's allowlist off for every stdio client. `main` cannot be tested,
    /// so the composition root is asserted directly on both transports.
    #[tokio::test]
    async fn stdio_composition_root_keeps_the_export_roots_contract() {
        let config = webfang_core::config::Config::default();
        let container = Arc::new(
            webfang_core::di::Container::new(config.crawler, config.scraper)
                .await
                .expect("container creation failed"),
        );
        let roots = vec![std::path::PathBuf::from("/srv/allowed")];

        let state = build_state(container, roots.clone()).expect("stdio state composes");
        assert_eq!(
            state.allowed_export_roots.as_slice(),
            roots.as_slice(),
            "stdio must honor --export-roots / WEBFANG_MCP_EXPORT_ROOTS (#696)"
        );
    }

    // -----------------------------------------------------------------------
    // #1611 F7 — the stdin per-frame cap
    // -----------------------------------------------------------------------
    //
    // These pin the WRAPPER, not the cap value (the value is justified in
    // `MAX_STDIN_LINE_BYTES`'s doc and shared with the `urls` cap's test in
    // `tests/mcp_params_validation_test.rs`). What must hold is the shape: a
    // frame under the limit is forwarded byte-for-byte, the count is per frame
    // and not per session, and crossing it is a typed `InvalidData` refusal
    // that raises the death signal `main()` acts on.

    /// A frame of exactly the cap is legal — the limit is inclusive, and
    /// `poll_read` refuses on `> max_line_bytes`, not `>=`.
    #[tokio::test]
    async fn bounded_stdin_accepts_a_frame_exactly_at_the_cap() {
        let (mut client, server) = tokio::io::duplex(8);
        let mut stdin = BoundedStdin::new(server, TransportDeathSignal::new(), 4);
        tokio::io::AsyncWriteExt::write_all(&mut client, b"abcd\n")
            .await
            .expect("write one frame");
        drop(client);

        let mut seen = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stdin, &mut seen)
            .await
            .expect("a frame exactly at the cap must pass through");
        assert_eq!(seen, b"abcd\n", "bytes must reach rmcp untouched");
    }

    /// The counter is per FRAME: a long session of legal frames is never
    /// throttled by its own history (the newline resets it).
    #[tokio::test]
    async fn bounded_stdin_counts_each_frame_separately() {
        let (mut client, server) = tokio::io::duplex(8);
        let mut stdin = BoundedStdin::new(server, TransportDeathSignal::new(), 8);
        tokio::io::AsyncWriteExt::write_all(&mut client, b"abcd\nab\n")
            .await
            .expect("write two frames");
        drop(client);

        let mut seen = Vec::new();
        tokio::io::AsyncReadExt::read_to_end(&mut stdin, &mut seen)
            .await
            .expect("two frames under the cap must both pass through");
        assert_eq!(seen, b"abcd\nab\n");
    }

    /// A frame with no newline in sight is refused as soon as it crosses the
    /// cap — the unbounded-allocation case rmcp's `line_buf` cannot stop.
    #[tokio::test]
    async fn bounded_stdin_refuses_an_unterminated_oversize_frame() {
        let (mut client, server) = tokio::io::duplex(1024);
        let signal = TransportDeathSignal::new();
        let observer = signal.clone();
        let mut stdin = BoundedStdin::new(server, signal, 16);
        let payload = vec![b'x'; 40];
        tokio::io::AsyncWriteExt::write_all(&mut client, &payload)
            .await
            .expect("write an oversize frame");
        drop(client);

        let mut seen = Vec::new();
        let err = tokio::io::AsyncReadExt::read_to_end(&mut stdin, &mut seen)
            .await
            .expect_err("a frame past the cap must be refused");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidData,
            "a cap refusal is bad input, not an I/O fault; got: {err}"
        );
        assert!(
            err.to_string().contains("16"),
            "the refusal must name the cap the operator configured; got: {err}"
        );
        // main() can only act on this through the signal: rmcp reports the read
        // error as a plain log line and ends the session as if it were EOF.
        let detail = observer
            .first_error_message()
            .expect("the death signal must carry the refusal reason");
        assert!(
            detail.contains("stdin cap"),
            "the signal must carry the reason; got: {detail}"
        );
    }

    /// The frame that JUST COMPLETED is the one rmcp is about to parse, so it
    /// is measured before the counter resets — an oversize line followed by a
    /// newline is still refused even though nothing is left over, and the
    /// refusal names THAT length (a regression once logged it as the empty
    /// remainder, "refused at 0 bytes").
    #[tokio::test]
    async fn bounded_stdin_refuses_the_completed_frame_not_the_remainder() {
        let (mut client, server) = tokio::io::duplex(1024);
        let mut stdin = BoundedStdin::new(server, TransportDeathSignal::new(), 10);
        let mut payload = vec![b'x'; 20];
        payload.push(b'\n');
        tokio::io::AsyncWriteExt::write_all(&mut client, &payload)
            .await
            .expect("write an oversize terminated frame");
        drop(client);

        let mut seen = Vec::new();
        let err = tokio::io::AsyncReadExt::read_to_end(&mut stdin, &mut seen)
            .await
            .expect_err("the completed frame's own length is what is capped");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData, "got: {err}");
        assert!(
            err.to_string().contains("refused at 20 bytes"),
            "the refusal must report the completed frame's length, not the empty \
             remainder after it; got: {err}"
        );
    }
}
