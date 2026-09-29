//! MCP Server — Axum router with Streamable HTTP transport
//!
//! Sets up the MCP server using rmcp's StreamableHttpService
//! mounted on an Axum router at /mcp, with a full middleware stack:
//! panic containment, panic hook, timeout, body limit, rate limiting,
//! session admission control and optional auth.

use std::any::Any;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{middleware, Router};
use governor::{
    clock::DefaultClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovernorLimiter,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, tower::StreamableHttpService,
};
use tokio_util::sync::CancellationToken;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::info;

use super::auth::{validate_auth, AuthState};
use super::panic_hook::setup_panic_hook;
use super::render_panic_payload;
use super::state::McpState;
use super::McpHandler;

/// Default address for the MCP server.
pub const DEFAULT_MCP_ADDR: &str = "127.0.0.1:8080";

/// Configuration for the MCP HTTP server middleware stack.
#[derive(Debug, Clone)]
pub struct ServerOptions {
    /// Request timeout in seconds (default: 30).
    pub request_timeout_secs: u64,
    /// Maximum request body size in bytes (default: 10 MB).
    pub body_limit_bytes: usize,
    /// Maximum requests per second (default: 10, matching the `webfang-mcp`
    /// HTTP binary's `--rate` default).
    pub rate_per_second: u32,
    /// Maximum burst size for rate limiting (default: 20, matching the
    /// `webfang-mcp` HTTP binary's `--burst` default).
    pub rate_burst: u32,
    /// Expected Bearer token. When `None`, auth is disabled.
    pub auth_token: Option<String>,
    /// Maximum number of session-creating requests admitted inside one
    /// [`Self::session_cap_window_secs`] window (default: 64, see
    /// [`DEFAULT_MAX_SESSIONS`]). Enforced by the session admission cap; see
    /// `SessionCap` for what "session-creating" means and what the cap does
    /// NOT know.
    ///
    /// A [`NonZeroUsize`], like every count limit in this crate: a cap of zero
    /// is a misconfiguration, not a mode (cf. `McpState::CategoryLimits`).
    pub max_sessions: NonZeroUsize,
    /// Lifetime of one admission slot, in seconds (default: 300, see
    /// [`DEFAULT_SESSION_CAP_WINDOW_SECS`]).
    ///
    /// Also [`NonZeroU64`]: a zero-length window would mean "a slot is never
    /// released", which is a footgun dressed as a mode.
    pub session_cap_window_secs: NonZeroU64,
}

impl Default for ServerOptions {
    fn default() -> Self {
        // The literals are non-zero by construction; the guard is the repo
        // idiom from `McpState::CategoryLimits` (never `expect` in production
        // code). An operator-supplied zero is a different question, and it is
        // answered at the CLI boundary by the fail-fast checks in the
        // `webfang-mcp` binary.
        let nz = |v: usize| {
            NonZeroUsize::new(v).unwrap_or_else(|| unreachable!("limit literal {v} is non-zero"))
        };
        let nz_secs = |v: u64| {
            NonZeroU64::new(v).unwrap_or_else(|| unreachable!("window literal {v} is non-zero"))
        };
        Self {
            request_timeout_secs: 30,
            body_limit_bytes: 10 * 1024 * 1024,
            rate_per_second: 10,
            rate_burst: 20,
            auth_token: None,
            max_sessions: nz(DEFAULT_MAX_SESSIONS),
            session_cap_window_secs: nz_secs(DEFAULT_SESSION_CAP_WINDOW_SECS),
        }
    }
}

/// Fail-fast guard for tokenless binds on non-loopback interfaces (REQ-06).
///
/// Binding the MCP server to a routable address without an auth token would
/// expose the scraper surface to the network; the binary must refuse to start
/// before building the container. Loopback binds stay token-free (dev mode)
/// and `IpAddr::is_loopback()` covers both `127.0.0.1` and `::1`.
///
/// # Errors
///
/// Returns a user-facing Spanish error naming the bind address when `bind`
/// is non-loopback and no auth token is present, pointing the operator at
/// `--auth-token` / `WEBFANG_MCP_AUTH_TOKEN`.
pub fn require_auth_for_external_bind(bind: SocketAddr, token_present: bool) -> anyhow::Result<()> {
    if !bind.ip().is_loopback() && !token_present {
        return Err(anyhow::anyhow!(
            "No se puede iniciar el servidor MCP en {bind} sin token de autenticación. Defina --auth-token o WEBFANG_MCP_AUTH_TOKEN."
        ));
    }
    Ok(())
}

/// Build the Axum router with MCP endpoint and full middleware stack.
///
/// This is the production composition root: it builds the rmcp
/// [`StreamableHttpService`] over [`McpHandler::new`] and hands it to
/// [`build_mcp_router_with_service`], which owns the whole middleware stack.
/// The signature is unchanged for every existing caller (the HTTP binary, the
/// test harness and the integration suites).
///
/// [`StreamableHttpService`]: rmcp::transport::streamable_http_server::tower::StreamableHttpService
pub fn build_mcp_router(state: McpState, options: &ServerOptions) -> Router {
    let service = StreamableHttpService::new(
        move || Ok(McpHandler::new(state.clone())),
        LocalSessionManager::default().into(),
        Default::default(),
    );

    build_mcp_router_with_service(service, options)
}

/// Mount an already-built MCP service under `/mcp` behind the full stack.
///
/// # Why the service is a parameter
///
/// The stack is the load-bearing part of the panic-containment design
/// (#1611, F2) and it must be provable on the REAL composition, not on a
/// hand-rolled replica — a test that mounted a different stack would prove
/// nothing. So the stack lives here and the `/mcp` service is injected. The
/// bounds are exactly what [`Router::nest_service`] requires, deliberately NOT
/// specialized to `StreamableHttpService<McpHandler, _>`: that would force the
/// seam back to a single concrete service, and the panic-to-JSON-RPC mapping
/// (`jsonrpc_panic_response`, module-private) could then only be exercised
/// through a live listener. Generic over the service, `tower::ServiceExt::oneshot`
/// can drive the identical stack in-process with a service that panics, which is
/// what pins the mapping deterministically.
///
/// Production callers keep using [`build_mcp_router`]; the generic parameter
/// exists for tests that must mount a different service (`StreamableHttpService`
/// over a handler carrying a test-only tool) or a deliberately panicking one.
///
/// # Protocol-shape answers we do not own
///
/// The `/mcp` service is rmcp's [`StreamableHttpService`], which answers
/// JSON-RPC envelope problems at the HTTP layer BEFORE any method dispatch, and
/// its status codes are not configurable from here (#1294 P5-1/P5-2):
///
/// | Client mistake | Answer |
/// | :--- | :--- |
/// | `Accept` lacking both `application/json` and `text/event-stream` | `406` |
/// | `Content-Type` not `application/json` | `415` |
/// | body that is not a JSON-RPC 2.0 message (includes a 1.0 body) | `415` |
/// | non-`initialize` request with no `mcp-session-id` | `422` |
///
/// Only after those gates does a request reach dispatch, where an unknown method
/// becomes a JSON-RPC `-32601` over HTTP 200. Each row is pinned by
/// `tests/mcp_transport_contract_test.rs`; changing one upstream breaks that
/// suite instead of silently changing what agents see. Deliberately not handled
/// here: a translating middleware layer, which would fight the framework on every
/// rmcp bump (design decision, #1294 slice A).
///
/// [`StreamableHttpService`]: rmcp::transport::streamable_http_server::tower::StreamableHttpService
pub fn build_mcp_router_with_service<S>(service: S, options: &ServerOptions) -> Router
where
    S: tower::Service<axum::http::Request<axum::body::Body>, Error = std::convert::Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Response: IntoResponse + 'static,
    S::Future: Send + 'static,
{
    let service = tower::ServiceExt::map_response(service, into_axum_response);

    let rate_limiter = build_rate_limiter(options);
    let auth_state = AuthState {
        expected_token: options.auth_token.clone().map(Arc::from),
    };
    let session_cap = Arc::new(SessionCap::new(
        options.max_sessions,
        Duration::from_secs(options.session_cap_window_secs.get()),
    ));

    Router::new()
        .nest_service(MCP_ENDPOINT_PATH, service)
        // Innermost layer, applied FIRST: `.layer()` wraps what is already
        // there, so this is the LAST gate a request meets. Auth, the rate
        // limiter, the timeout and the body limit have all answered by now —
        // which is the security property that matters here: a request rejected
        // 401 (no/invalid token) or shed 429 by the rate limiter can never
        // consume a session slot, so an unauthenticated flood cannot exhaust
        // the session budget of the legitimate operator.
        .layer(middleware::from_fn_with_state(
            session_cap,
            session_cap_middleware,
        ))
        .layer(middleware::from_fn_with_state(auth_state, validate_auth))
        .layer(middleware::from_fn_with_state(
            rate_limiter,
            rate_limit_middleware,
        ))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(options.request_timeout_secs),
        ))
        .layer(RequestBodyLimitLayer::new(options.body_limit_bytes))
        .layer(TraceLayer::new_for_http())
        // Outermost: applied LAST, so it wraps auth, rate limiting, timeout,
        // body limit, tracing and the nested service. Any panic still escaping
        // an inner layer becomes a JSON-RPC error body instead of tearing the
        // connection down. A tool-handler panic is contained one level deeper,
        // in `McpHandler::call_tool` (#1611, F2).
        .layer(CatchPanicLayer::custom(jsonrpc_panic_response))
}

/// Erase the concrete response body type: axum routers and the tower-http
/// layers above are all monomorphic in `Response<Body>`, and `nest_service`
/// only requires the `IntoResponse` contract.
fn into_axum_response<R: IntoResponse>(response: R) -> Response {
    response.into_response()
}

/// JSON-RPC `Internal error` code (JSON-RPC 2.0 spec) — what a contained panic
/// maps to.
const JSONRPC_INTERNAL_ERROR: i64 = -32603;

/// User-facing (Spanish) body of a contained panic. Fixed text: the panic
/// payload may embed request data, so it goes to the trace, never to the client.
const PANIC_HTTP_ERROR: &str =
    "Error interno del servidor MCP contenido. La petición falló de forma inesperada.";

/// Map a panic raised anywhere on the HTTP request path to a JSON-RPC
/// `-32603` error body (HTTP 500, `application/json`).
///
/// Named (not a closure) so the contract is unit-testable: the mapping is the
/// only thing an agent sees when a panic escapes the inner layers, and
/// tower-http's own default handler answers with an EMPTY body, which an agent
/// reads as a transport failure rather than a server error.
///
/// A named `fn` is also what satisfies
/// [`CatchPanicLayer::custom`]: the handler must be `FnMut + Clone`, so a
/// closure would have to be written as an explicitly cloneable wrapper.
fn jsonrpc_panic_response(payload: Box<dyn Any + Send + 'static>) -> Response {
    tracing::error!(
        panic.payload = %render_panic_payload(payload.as_ref()),
        "MCP request panicked — mapped to a JSON-RPC -32603 response"
    );

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": serde_json::Value::Null,
        "error": {
            "code": JSONRPC_INTERNAL_ERROR,
            "message": PANIC_HTTP_ERROR,
        },
    });

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Build a `governor` rate limiter from [`ServerOptions`].
///
/// A direct (unkeyed) limiter applies one global quota across all requests.
fn build_rate_limiter(
    options: &ServerOptions,
) -> Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>> {
    let per_second = NonZeroU32::new(options.rate_per_second).unwrap_or(NonZeroU32::MIN);
    let burst = NonZeroU32::new(options.rate_burst).unwrap_or(NonZeroU32::MIN);
    let quota = Quota::per_second(per_second).allow_burst(burst);
    Arc::new(GovernorLimiter::direct(quota))
}

/// Rate limiting middleware — rejects requests exceeding the quota with 429.
async fn rate_limit_middleware(
    State(limiter): State<Arc<GovernorLimiter<NotKeyed, InMemoryState, DefaultClock>>>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Result<impl axum::response::IntoResponse, axum::http::StatusCode> {
    match limiter.check() {
        Ok(()) => Ok(next.run(request).await),
        Err(_not_until) => {
            tracing::warn!(
                remote = %request.uri().path(),
                "rate limit exceeded — rejecting with 429"
            );
            Err(axum::http::StatusCode::TOO_MANY_REQUESTS)
        },
    }
}

// ---------------------------------------------------------------------------
// Session admission control (#1611, F6)
// ---------------------------------------------------------------------------

/// Path the MCP service is nested at (see `build_mcp_router_with_service`).
pub const MCP_ENDPOINT_PATH: &str = "/mcp";

/// Default number of session-creating requests admitted per window (see
/// [`DEFAULT_SESSION_CAP_WINDOW_SECS`]).
///
/// 64 is roughly an order of magnitude above the fleet a single-instance MCP
/// server actually serves (one session per agent host, plus reconnects), while
/// still bounding what an unbounded session map can cost: each session holds a
/// spawned worker, two 16-slot channels and a cloned `McpHandler`. Above the
/// rate limiter's default (10 rps, burst 20) it takes seconds of sustained
/// `initialize` traffic to fill, so the cap sheds abuse instead of throttling
/// honest reconnects. Operators with a larger fleet raise it
/// (`--max-sessions` / `WEBFANG_MCP_MAX_SESSIONS`).
pub const DEFAULT_MAX_SESSIONS: usize = 64;

/// Default admission window, in seconds.
///
/// 300 is rmcp's own `SessionConfig::keep_alive` default, and that is exactly
/// why: a slot is released one keep-alive period after the request that took
/// it, which is the longest the session it accounts for can still be alive.
/// A longer window would hold slots for sessions rmcp has already reaped; a
/// shorter one would free slots belonging to live sessions.
pub const DEFAULT_SESSION_CAP_WINDOW_SECS: u64 = 300;

/// Header rmcp uses to bind a request to an existing session. The literal is
/// mirrored here because rmcp keeps `HEADER_SESSION_ID` crate-private
/// (`transport/common/http_header.rs`); it is the same literal the transport
/// emits and the test harness sends.
const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// `operation` field of every event this cap emits — the resource it governs.
const SESSION_OPERATION: &str = "mcp.session.create";

/// Spanish sentence for the operator to hand to whoever is being shed, carried
/// as the `user_message` field of the rejection event (see
/// `session_cap_middleware` for why it is not an HTTP body).
const SESSION_CAP_USER_MESSAGE: &str =
    "Límite de sesiones MCP alcanzado. Inténtelo de nuevo en unos segundos.";

/// Whether `path` addresses the nested MCP endpoint (`/mcp` or `/mcp/…`).
///
/// The cap is mounted on the whole router, so without this a POST to any
/// other path would burn a session slot — and answering a 404 with 429 is how
/// a scanner locks a legitimate operator out of its own server.
fn is_mcp_endpoint(path: &str) -> bool {
    path == MCP_ENDPOINT_PATH
        || path
            .strip_prefix(MCP_ENDPOINT_PATH)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether this request is one rmcp will turn into a new session.
///
/// Deliberately NOT "the body says `initialize`". The predicate mirrors
/// rmcp 1.8.0's own allocation trigger: `handle_post` calls
/// `session_manager.create_session()` in exactly one place (`tower.rs:1129`),
/// the `else` arm of "do we have a session id?" — a POST to `/mcp` with no
/// `mcp-session-id` header. Everything else (a session-bearing POST, GET,
/// DELETE) rides an existing session or allocates nothing.
fn creates_session(request: &axum::http::Request<axum::body::Body>) -> bool {
    request.method() == Method::POST
        && !request.headers().contains_key(MCP_SESSION_ID_HEADER)
        && is_mcp_endpoint(request.uri().path())
}

/// Admission control for Streamable HTTP sessions (#1611, F6).
///
/// # The finding
///
/// `build_mcp_router` composes rmcp's `LocalSessionManager::default()`
/// (`session/local.rs:30-34`), whose only knobs are `SessionConfig`'s timeouts
/// and channel capacity: `keep_alive` bounds the life of ONE session, never how
/// many exist, and neither `SessionManager` nor `StreamableHttpServerConfig`
/// carries a count anywhere in the crate. A client can therefore open sessions
/// without bound.
///
/// # Why a middleware and not a `SessionManager` wrapper
///
/// The semantically correct shape is a newtype implementing `SessionManager`,
/// delegating to `LocalSessionManager` and refusing `create_session` past the
/// cap by reading its public `sessions` map: it would count REAL sessions and a
/// DELETE would free its slot on the spot. It is deliberately not this slice,
/// because rmcp maps a `create_session` error to **HTTP 500 with a plain-text
/// body** (`internal_error_response`, `transport/common/server_side_http.rs`)
/// — a load-shedding condition answered as a server fault, with no JSON-RPC
/// envelope to explain it. A middleware answers 429 at the HTTP layer, next to
/// the rate limiter that already rejects with a bare `StatusCode`. Exact
/// per-session accounting is the follow-up slice.
///
/// # What is counted, and why
///
/// The counter is charged by `creates_session`, i.e. by rmcp's own allocation
/// trigger (see that function). Counting `initialize` *bodies* instead would
/// leave the resource uncapped through a path this crate does not own: rmcp
/// allocates the session at `tower.rs:1129` and only THEN checks that the
/// message is an initialize request (`tower.rs:1146`), so a session-less POST
/// of anything else is answered `422` and still leaves an entry in the session
/// map — whose worker gives up on `init_timeout` without the manager ever
/// removing it. The predicate therefore charges the requests that allocate,
/// and over-charges the ones rmcp rejects at its own gates (406 / 415 / 422) —
/// over-counting sheds load, never admits it.
///
/// # What this cap does NOT know (all three accepted, none hidden)
///
/// 1. **It counts requests, not live sessions.** For a well-behaved client the
///    two coincide — every re-`initialize` allocates a second session and
///    never reuses the first — so a client that re-initializes is charged for
///    work it really did. The divergence is only in the safe direction:
///    malformed traffic is charged for sessions that were never created.
/// 2. **A DELETE does not free its slot.** rmcp closes the session at once; the
///    slot is released when the window slides. Bounded by construction: a slot
///    outlives its request by at most the window, which defaults to the
///    session's own maximum lifetime, so the only over-hold is a client that
///    closed a session early and keeps its slot for the rest of the window.
///    Freeing it exactly would need the session id of each admission — the
///    `SessionManager` follow-up's job.
/// 3. **A client that re-initializes periodically does spend budget.** Steady
///    state is (re-initializations per second × window) slots, so the defaults
///    admit one reconnect every ~4.7s indefinitely; a client that reconnects
///    faster than that is doing exactly what the cap exists to bound. Raise
///    `max_sessions` for a larger fleet.
///
/// # One honest limitation
///
/// Admission is a check-then-act on a shared counter, so N requests racing at
/// the same instant can admit up to N slots. There is no atomic reservation
/// across the request boundary, and there is no later eviction to undo it, so
/// concurrent overshoot past `max_sessions` is possible and accepted; the
/// per-window expiry is what bounds it in time.
struct SessionCap {
    /// Slots available inside one window.
    max: NonZeroUsize,
    /// Lifetime of a single slot, measured from the admission that took it.
    window: Duration,
    /// Admission instants currently holding a slot, oldest first. Bounded by
    /// `max`: a rejected request never pushes, so the deque cannot grow past
    /// the cap it enforces.
    admitted: Mutex<VecDeque<Instant>>,
}

impl SessionCap {
    /// Build a cap of `max` slots, each living `window`.
    fn new(max: NonZeroUsize, window: Duration) -> Self {
        Self {
            max,
            window,
            admitted: Mutex::new(VecDeque::with_capacity(max.get())),
        }
    }

    /// Charge one slot at `now`, or report how many are held.
    ///
    /// `Ok(held)` is the count AFTER the admission; `Err(held)` is the count
    /// that refused it. The whole decision happens inside one short
    /// synchronous section, so no lock is ever held across an `.await`.
    ///
    /// `now` is a parameter rather than `Instant::now()` so the eviction
    /// boundary is testable exactly instead of approximately after a sleep —
    /// there is one production caller and it passes the wall clock.
    fn try_admit(&self, now: Instant) -> Result<usize, usize> {
        let mut admitted = self.lock();
        while admitted
            .front()
            .is_some_and(|at| now.duration_since(*at) >= self.window)
        {
            admitted.pop_front();
        }
        let held = admitted.len();
        if held >= self.max.get() {
            return Err(held);
        }
        admitted.push_back(now);
        Ok(held + 1)
    }

    /// Slots currently held — test-only view, without pruning.
    #[cfg(test)]
    fn held(&self) -> usize {
        self.lock().len()
    }

    /// Recover from a poisoned mutex instead of propagating the panic.
    ///
    /// The critical section only prunes and pushes one timestamp, so a panic
    /// inside it can leave a stale entry but never a corrupted counter —
    /// whereas propagating the poison would turn one panicking request into a
    /// cap that is broken for the life of the process.
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Instant>> {
        self.admitted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Session admission middleware — sheds session-creating requests past the cap
/// with 429, and charges nothing for traffic on an established session.
///
/// Rejection is a bare [`StatusCode::TOO_MANY_REQUESTS`], exactly like
/// `rate_limit_middleware`: load shedding is an HTTP-layer answer, and the
/// repo does not answer it with a JSON-RPC envelope. The reason a client was
/// shed therefore reaches the operator and not the agent — as the Spanish
/// `user_message` field of the event below (tracing field names and event
/// messages stay English; the sentence a human hands to a user is Spanish).
///
/// `log_scrape_error` is deliberately NOT used: its shape is an error VALUE
/// plus a URL, a stage and a correlation id for a scrape failure, and a shed
/// admission is neither — there is no error, no URL and no scrape to correlate.
/// A structured event at the decision point, carrying the limit, the live count
/// and the operation, is the honest shape.
async fn session_cap_middleware(
    State(cap): State<Arc<SessionCap>>,
    request: axum::http::Request<axum::body::Body>,
    next: middleware::Next,
) -> Result<impl axum::response::IntoResponse, axum::http::StatusCode> {
    if !creates_session(&request) {
        return Ok(next.run(request).await);
    }

    match cap.try_admit(Instant::now()) {
        Ok(held) => {
            tracing::debug!(
                operation = SESSION_OPERATION,
                limit = cap.max.get(),
                admitted = held,
                window_secs = cap.window.as_secs(),
                "session admitted within the admission cap"
            );
            Ok(next.run(request).await)
        },
        Err(held) => {
            tracing::warn!(
                operation = SESSION_OPERATION,
                limit = cap.max.get(),
                admitted = held,
                window_secs = cap.window.as_secs(),
                user_message = SESSION_CAP_USER_MESSAGE,
                "session admission cap exceeded — rejecting the request with 429"
            );
            Err(axum::http::StatusCode::TOO_MANY_REQUESTS)
        },
    }
}

/// Start the MCP server on the given address with the full middleware stack.
///
/// # Connection Pooling
///
/// For production use, create a shared `Downloader` and inject it via
/// `McpState::with_downloader()`. Long-lived servers MUST use the bounded
/// composition-root helper (#1120), never the legacy unbounded
/// `Downloader::new`:
///
/// ```rust,ignore
/// use std::sync::Arc;
/// use webfang_mcp::mcp_server::{build_shared_downloader, McpState};
///
/// let downloader = Arc::new(build_shared_downloader()?);
/// let state = McpState::new(container).with_downloader(downloader);
/// start_mcp_server(state, addr, ServerOptions::default()).await?;
/// ```
///
/// Without a shared Downloader, each MCP tool call creates a fresh connection pool,
/// defeating keep-alive and TLS session reuse.
///
/// # Errors
///
/// Returns an error if the TCP listener cannot bind to `addr`, or if the
/// server fails while serving.
pub async fn start_mcp_server(
    state: McpState,
    addr: SocketAddr,
    options: ServerOptions,
) -> anyhow::Result<()> {
    setup_panic_hook();

    let app = build_mcp_router(state.clone(), &options);

    info!("MCP server starting on http://{}/mcp", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    // Capture the serve result instead of `?`: a serve error must still drain
    // the writer below — the early-return would silently skip the flush of
    // every buffered record (#1143 review). The failure is propagated after
    // the drain completes.
    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(state.cancel_token.clone()))
        .await;

    // #1121: drain and join the crawl-result background writer before the
    // runtime goes away, so every write acknowledged by `save` is confirmed
    // persisted and a dead writer is reported instead of silently detached.
    if let Some(repo) = state.container.crawl_result_repository() {
        if let Err(e) = repo.shutdown().await {
            tracing::warn!(error = %e, "crawl-result writer shutdown reported errors");
        }
    }

    serve_result?;

    Ok(())
}

/// Future that resolves when a shutdown is requested (Ctrl+C or SIGTERM).
///
/// Drives the OS signal handler in the background and cancels the supplied
/// [`CancellationToken`] on Ctrl+C. Returns a `'static` future (it owns its
/// own clone of the token) so it is suitable for
/// `axum::serve(...).with_graceful_shutdown(...)`.
async fn shutdown_signal(token: CancellationToken) {
    if let Err(e) = tokio::signal::ctrl_c().await {
        tracing::warn!(
            error = %e,
            "SIGINT handler unavailable — server will keep running and rely on SIGTERM"
        );
        // Without a working Ctrl+C handler, wait on the token directly so the
        // server can still be torn down via an explicit cancel().
        token.cancelled().await;
        return;
    }
    info!("MCP server shutting down");
    token.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use webfang_core::config::Config;
    use webfang_core::di::{Container, ContainerExt};

    /// The contained-panic mapping contract is asserted above; this test makes
    /// the whole stack panic-safe in-process: a service that panics on the HTTP
    /// request path, driven through the REAL stack with `oneshot` (no
    /// listener, no sleep, no network).
    #[tokio::test]
    async fn panic_on_the_http_path_is_mapped_by_the_real_stack() {
        use tower::ServiceExt;

        async fn panicking_route(
            _req: axum::http::Request<axum::body::Body>,
        ) -> Result<Response, std::convert::Infallible> {
            panic!("probe: panic on the HTTP request path");
        }

        let app = build_mcp_router_with_service(
            tower::service_fn(panicking_route),
            &ServerOptions::default(),
        );
        let request = axum::http::Request::builder()
            .uri("/mcp")
            .body(axum::body::Body::empty())
            .expect("valid request");

        let response = app
            .oneshot(request)
            .await
            .expect("CatchPanicLayer turns the panic into a response");

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let text = String::from_utf8_lossy(&body);
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("mapped body is JSON");
        assert_eq!(parsed["error"]["code"], JSONRPC_INTERNAL_ERROR);
        assert!(
            !text.contains("HTTP request path"),
            "the panic payload must not leak through the stack: {text}"
        );
    }

    /// Build a test McpHandler with DI container.
    async fn test_handler() -> McpHandler {
        let config = Config::default();
        let container = Container::from_config(config).await.unwrap();
        let state = McpState::new(container);
        McpHandler::new(state)
    }

    #[cfg_attr(
        miri,
        ignore = "Container::new creates HttpClient with boring-sys2 FFI (unsupported by Miri)"
    )]
    #[tokio::test]
    async fn test_handler_builds_with_all_tools() {
        let handler = test_handler().await;
        let tools = handler.tool_router.list_all();
        assert!(
            tools.len() >= 35,
            "Expected at least 35 tools, got {}",
            tools.len()
        );

        let tool_names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(tool_names.contains(&"scrape_url"));
        assert!(tool_names.contains(&"validate_url"));
        assert!(tool_names.contains(&"clean_html"));
        assert!(tool_names.contains(&"detect_waf"));
        assert!(tool_names.contains(&"download_assets"));
        assert!(tool_names.contains(&"extract_domain"));
        assert!(tool_names.contains(&"normalize_url"));
        assert!(tool_names.contains(&"convert_html_to_markdown"));
    }

    /// Test tool logic by calling the underlying functions directly
    /// (bypasses MCP protocol layer which requires peer/session setup).

    #[test]
    fn test_validate_url_logic() {
        let url = url::Url::parse("https://example.com/path?q=1").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(url.path(), "/path");
    }

    #[test]
    fn test_normalize_url_logic() {
        let mut url = url::Url::parse("https://example.com/path/#fragment").unwrap();
        url.set_fragment(None);
        let path = url.path().trim_end_matches('/').to_string();
        url.set_path(&path);
        let result = url.to_string();
        assert!(!result.contains("#fragment"));
        assert!(!result.ends_with("/"));
    }

    #[test]
    fn test_extract_domain_logic() {
        let url = url::Url::parse("https://www.example.com/path").unwrap();
        assert_eq!(url.host_str(), Some("www.example.com"));
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_clean_html_logic() {
        let html = "<html><head><script>alert('x')</script></head><body><p>Hello</p></body></html>";
        let cleaned = webfang_core::infrastructure::converter::html_cleaner::clean_html(html);
        assert!(!cleaned.contains("script"));
        assert!(cleaned.contains("Hello"));
    }

    #[test]
    fn test_convert_html_to_markdown_logic() {
        let html = "<h1>Title</h1><p>Paragraph</p>";
        let md =
            webfang_core::infrastructure::converter::html_to_markdown::convert_to_markdown(html);
        assert!(md.contains("Title"));
        assert!(md.contains("Paragraph"));
    }

    #[test]
    fn test_waf_detector_logic() {
        use webfang_core::domain::waf::InspectionContext;
        use webfang_core::infrastructure::http::waf_engine::WafInspector;
        let clean_html = "<html><body>Normal content</body></html>";
        let verdict = WafInspector::inspect(clean_html, &InspectionContext::default());
        assert!(!verdict.is_blocked);
    }

    #[test]
    fn test_waf_detector_cloudflare() {
        use webfang_core::domain::waf::InspectionContext;
        use webfang_core::infrastructure::http::waf_engine::WafInspector;
        let cf_html = "<div id=\"cf-turnstile\" data-sitekey=\"abc123\"></div>";
        let verdict = WafInspector::inspect(cf_html, &InspectionContext::default());
        assert!(verdict.is_blocked);
        assert!(verdict
            .evidences
            .first()
            .is_some_and(|e| e.provider.contains("Cloudflare")));
    }

    #[test]
    fn test_output_path_logic() {
        let path =
            webfang_core::adapters::url_path::OutputPath::from_url("https://example.com/docs/page")
                .unwrap();
        let full = path.to_full_path();
        assert!(full.contains("example.com"));
        assert!(full.contains("docs"));
    }

    #[test]
    fn test_frontmatter_generation() {
        let fm = webfang_core::infrastructure::output::frontmatter::generate(
            "Test Title",
            "https://example.com",
            None,
            None,
            None,
            &[],
        );
        assert!(fm.contains("Test Title"));
        assert!(fm.contains("example.com"));
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_highlight_code_blocks_logic() {
        let md = "```rust\nfn main() {}\n```";
        let highlighted =
            webfang_core::infrastructure::converter::syntax_highlight::highlight_code_blocks(md);
        // Syntax highlighting may or may not add markup; just verify it returns something
        assert!(!highlighted.is_empty());
    }

    #[test]
    fn test_convert_wiki_links_logic() {
        let md = "https://example.com/page";
        let wikilinks = webfang_core::infrastructure::converter::wikilinks::convert_wiki_links(
            md,
            "example.com",
        );
        // Wiki link conversion replaces same-domain URLs with [[page]] syntax
        assert!(!wikilinks.is_empty());
    }

    #[test]
    fn test_mcp_state_with_downloader() {
        use std::sync::Arc;

        let config = Config::default();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let container = rt
            .block_on(Container::new(config.crawler, config.scraper))
            .unwrap();

        let tmp = tempfile::tempdir().unwrap();
        let dl_config = webfang_core::adapters::downloader::DownloadConfig {
            output_dir: tmp.path().to_path_buf(),
            ..Default::default()
        };
        let downloader =
            Arc::new(webfang_core::adapters::downloader::Downloader::new(dl_config).unwrap());
        let downloader_clone = downloader.clone();

        let state = McpState::new(container).with_downloader(downloader);
        assert!(
            state.downloader.is_some(),
            "McpState must hold the shared Downloader after with_downloader()"
        );
        assert!(
            Arc::ptr_eq(state.downloader.as_ref().unwrap(), &downloader_clone),
            "with_downloader must store the exact Arc (connection pool identity)"
        );

        // Clone preserves the shared pool
        let state2 = state.clone();
        assert!(
            state2.downloader.is_some(),
            "clone must preserve downloader"
        );
        assert!(
            Arc::ptr_eq(
                state.downloader.as_ref().unwrap(),
                state2.downloader.as_ref().unwrap()
            ),
            "cloned McpState must share the same Downloader Arc"
        );
    }

    #[test]
    fn test_server_options_default() {
        let opts = ServerOptions::default();
        assert_eq!(opts.request_timeout_secs, 30);
        assert_eq!(opts.body_limit_bytes, 10 * 1024 * 1024);
        assert_eq!(opts.rate_per_second, 10);
        assert_eq!(opts.rate_burst, 20);
        assert!(opts.auth_token.is_none());
        assert_eq!(opts.max_sessions.get(), DEFAULT_MAX_SESSIONS);
        assert_eq!(
            opts.session_cap_window_secs.get(),
            DEFAULT_SESSION_CAP_WINDOW_SECS
        );
    }

    // ------------------------------------------------------------------
    // F6 session admission cap (#1611)
    // ------------------------------------------------------------------

    /// The defaults are load-bearing for every other suite: `ServerOptions::default()`
    /// is what all the integration tests build, so a default low enough to shed
    /// honest traffic would fail the whole crate, and a default high enough to
    /// never bind would silently disable the cap. Pinned here, next to the
    /// constants, because both are otherwise only observable through a 429.
    #[test]
    fn session_cap_defaults_bound_a_real_fleet() {
        assert_eq!(DEFAULT_MAX_SESSIONS, 64);
        // 300 is rmcp's `SessionConfig::keep_alive` default, which is the whole
        // justification for the number (see the constant's doc comment).
        assert_eq!(DEFAULT_SESSION_CAP_WINDOW_SECS, 300);
    }

    fn cap(max: usize, window_secs: u64) -> SessionCap {
        SessionCap::new(
            NonZeroUsize::new(max).expect("test max is non-zero"),
            Duration::from_secs(window_secs),
        )
    }

    /// The cap admits exactly `max` and then refuses, and the refusal carries
    /// the count that produced it.
    #[test]
    fn try_admit_refuses_past_the_cap_and_reports_the_live_count() {
        let cap = cap(3, 300);
        let t0 = Instant::now();
        assert_eq!(cap.try_admit(t0), Ok(1));
        assert_eq!(cap.try_admit(t0), Ok(2));
        assert_eq!(cap.try_admit(t0), Ok(3));
        assert_eq!(cap.try_admit(t0), Err(3));
        assert_eq!(cap.try_admit(t0), Err(3));
        assert_eq!(cap.held(), 3, "a refused request must not consume a slot");
    }

    /// A slot is released when ITS window slides, not when the counter as a
    /// whole rolls over: a fixed-window counter would let one client admit
    /// `2 × max` by straddling a boundary. Two admissions 1s apart in a 1s
    /// window therefore shed the second and admit the first's replacement.
    ///
    /// Time is injected (`try_admit` takes the instant), so this is exact at
    /// the eviction boundary instead of approximate after a sleep.
    #[test]
    fn a_slot_is_released_when_its_own_window_slides() {
        let cap = cap(1, 1);
        let t0 = Instant::now();
        assert_eq!(cap.try_admit(t0), Ok(1));
        assert_eq!(cap.try_admit(t0 + Duration::from_millis(999)), Err(1));
        assert_eq!(
            cap.try_admit(t0 + Duration::from_secs(1)),
            Ok(1),
            "an admission exactly one window old is expired"
        );
    }

    /// Pruning releases only the AGED-OUT admissions: a fresh one survives, so
    /// the cap measures a sliding window and not "everything since the last
    /// full reset". (Clearing the deque on prune would pass the test above and
    /// fail here.)
    #[test]
    fn pruning_releases_only_the_aged_out_admissions() {
        let cap = cap(2, 10);
        let t0 = Instant::now();
        assert_eq!(cap.try_admit(t0), Ok(1));
        assert_eq!(cap.try_admit(t0 + Duration::from_secs(6)), Ok(2));

        // t=12s: the first admission (12s old) expired, the second (6s old)
        // did not — so exactly one slot came back.
        assert_eq!(cap.try_admit(t0 + Duration::from_secs(12)), Ok(2));
        assert_eq!(cap.try_admit(t0 + Duration::from_secs(12)), Err(2));
    }

    /// The predicate is the contract with rmcp: charge the requests that make
    /// it call `create_session`, and nothing else. Each row is a real traffic
    /// class, so a change here changes who pays for a session.
    #[test]
    fn creates_session_matches_only_rmcps_own_allocation_trigger() {
        let build = |method: &str, path: &str, session: bool| {
            let mut builder = axum::http::Request::builder().method(method).uri(path);
            if session {
                builder = builder.header(MCP_SESSION_ID_HEADER, "s-1");
            }
            builder
                .body(axum::body::Body::empty())
                .expect("valid request")
        };

        // The allocation trigger itself.
        assert!(creates_session(&build("POST", "/mcp", false)));
        assert!(creates_session(&build("POST", "/mcp/", false)));
        // Established-session traffic: rides a session, allocates nothing.
        assert!(!creates_session(&build("POST", "/mcp", true)));
        // GET opens the SSE stream of an existing session; DELETE closes one.
        assert!(!creates_session(&build("GET", "/mcp", false)));
        assert!(!creates_session(&build("DELETE", "/mcp", false)));
        // Off-endpoint POSTs must not burn a slot (a scanner could otherwise
        // lock the operator out with requests that allocate nothing).
        assert!(!creates_session(&build("POST", "/", false)));
        assert!(!creates_session(&build("POST", "/mcp-not", false)));
        assert!(!creates_session(&build("POST", "/other", false)));
    }

    /// The user-facing sentence is Spanish, like every other message a human
    /// reads out of this server (`panic_messages_are_spanish` pins the same
    /// convention for the contained-panic bodies).
    #[test]
    fn session_cap_user_message_is_spanish() {
        let has_spanish_char = SESSION_CAP_USER_MESSAGE
            .chars()
            .any(|c| matches!(c, 'ñ' | 'á' | 'é' | 'í' | 'ó' | 'ú'));
        assert!(
            SESSION_CAP_USER_MESSAGE.contains("Límite") && has_spanish_char,
            "the operator-facing sentence must be Spanish, got: {SESSION_CAP_USER_MESSAGE}"
        );
    }

    #[test]
    fn test_rate_limiter_allows_within_quota() {
        let opts = ServerOptions {
            rate_per_second: 10,
            rate_burst: 5,
            ..Default::default()
        };
        let limiter = build_rate_limiter(&opts);
        for _ in 0..5 {
            assert!(limiter.check().is_ok());
        }
    }

    #[test]
    fn test_rate_limiter_rejects_over_burst() {
        let opts = ServerOptions {
            rate_per_second: 1,
            rate_burst: 2,
            ..Default::default()
        };
        let limiter = build_rate_limiter(&opts);
        // Exhaust the burst capacity
        assert!(limiter.check().is_ok());
        assert!(limiter.check().is_ok());
        // Third request should be rejected
        assert!(limiter.check().is_err());
    }

    #[tokio::test]
    async fn test_cancel_token_propagates_to_clones() {
        let config = Config::default();
        let container = Container::from_config(config).await.unwrap();
        let state = McpState::new(container);
        let state2 = state.clone();

        // Cancel through one clone, observe via the other.
        state.shutdown_signal();
        assert!(state2.cancel_token.is_cancelled());
    }

    /// REQ-06: loopback binds stay token-free by default (dev mode).
    #[test]
    fn require_auth_loopback_no_token_is_ok() {
        let bind: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        assert!(require_auth_for_external_bind(bind, false).is_ok());
    }

    /// REQ-06: IPv6 loopback (`::1`) counts as loopback.
    #[test]
    fn require_auth_loopback_v6_no_token_is_ok() {
        let bind: SocketAddr = "[::1]:8080".parse().unwrap();
        assert!(require_auth_for_external_bind(bind, false).is_ok());
    }

    /// REQ-06: tokenless non-loopback binds fail fast with a Spanish message
    /// that names the bind address and the token options.
    #[test]
    fn require_auth_non_loopback_no_token_is_err() {
        for addr in ["0.0.0.0:8080", "192.168.1.10:8080"] {
            let bind: SocketAddr = addr.parse().unwrap();
            match require_auth_for_external_bind(bind, false) {
                Err(e) => {
                    let msg = e.to_string();
                    assert!(msg.contains(addr), "message must name the bind: {msg}");
                    assert!(
                        msg.contains("token"),
                        "message must mention the token: {msg}"
                    );
                },
                Ok(()) => panic!("{addr} must be rejected without a token"),
            }
        }
    }

    /// REQ-06: a token present lifts the non-loopback restriction.
    #[test]
    fn require_auth_non_loopback_with_token_is_ok() {
        let bind: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert!(require_auth_for_external_bind(bind, true).is_ok());
    }

    // ------------------------------------------------------------------
    // F2 panic containment (#1611) — the HTTP-layer mapping
    // ------------------------------------------------------------------

    /// The mapping is a fixed JSON-RPC `-32603` document: HTTP 500,
    /// `application/json`, and NOT the empty body tower-http answers by
    /// default (which an agent reads as a dead transport, not a server error).
    ///
    /// The payload is a secret-shaped string on purpose: it must not appear
    /// anywhere in the answer. The trace owns the payload, the client owns a
    /// sentence.
    #[tokio::test]
    async fn panic_mapping_answers_jsonrpc_internal_error_without_payload() {
        let response =
            jsonrpc_panic_response(Box::new(String::from("probe payload: sk-do-not-leak")));

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json"),
            "the contained-panic body must be JSON, not an empty/text fallback"
        );

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("mapping body reads");
        let text = String::from_utf8_lossy(&body);
        let parsed: serde_json::Value =
            serde_json::from_slice(&body).expect("mapping body is JSON");
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert!(
            parsed["id"].is_null(),
            "an unknown request has no id: {parsed}"
        );
        assert_eq!(parsed["error"]["code"], JSONRPC_INTERNAL_ERROR);
        assert_eq!(parsed["error"]["message"], PANIC_HTTP_ERROR);
        assert!(
            !text.contains("sk-do-not-leak"),
            "the panic payload must never reach the client: {text}"
        );
    }

    /// The user-facing text is Spanish, per the project convention for
    /// user-facing errors (tracing fields and code comments stay English).
    ///
    /// Pinned on markers an English sentence cannot contain (the article `La`
    /// and a Spanish-specific character — `ñ`, or an accented vowel): an "is
    /// it non-empty / is it lowercase" check would pass on the English
    /// translation of the same sentence, so it would pin nothing.
    #[test]
    fn panic_messages_are_spanish() {
        for (label, message) in [
            ("http", PANIC_HTTP_ERROR),
            ("tool", crate::mcp_server::PANIC_CONTAINED_TOOL_ERROR),
        ] {
            let has_spanish_char = message
                .chars()
                .any(|c| matches!(c, 'ñ' | 'á' | 'é' | 'í' | 'ó' | 'ú'));
            assert!(
                message.contains("La ") && has_spanish_char,
                "the {label} message must be Spanish, got: {message}"
            );
        }
    }

    /// `render_panic_payload` must be total: it never panics, never returns
    /// more than its bound, and says so plainly for a non-string payload
    /// (`panic_any` with a custom type is legal).
    #[test]
    fn render_panic_payload_is_bounded_and_total() {
        assert_eq!(render_panic_payload(&String::from("boom")), "boom");
        assert_eq!(render_panic_payload(&"boom"), "boom");
        assert_eq!(render_panic_payload(&42_u32), "<non-string panic payload>");

        let long = "x".repeat(5_000);
        let rendered = render_panic_payload(&long);
        assert!(
            rendered.chars().count() <= 201,
            "the rendered payload must stay bounded, got {} chars",
            rendered.chars().count()
        );
    }

    /// The bound is a CHARACTER bound on a multibyte payload (#1626, PC-2).
    ///
    /// The truncation index has to land on a char boundary, or the slice would
    /// not be valid UTF-8 and the renderer would panic — while reporting a
    /// panic. A megabyte of multibyte characters is also what makes the gap
    /// between the old full `chars().count()` walk and the bounded one enormous
    /// in bytes while identical in output, which is why the input is
    /// megabyte-scale on purpose.
    ///
    /// What this does NOT pin: that the walk is bounded. Complexity is not
    /// observable from a unit test without a timing assertion, and a timing
    /// assertion on a panic path is exactly the flake that hides a real
    /// regression. The bounded walk is justified by construction
    /// (`char_indices().nth(MAX_CHARS)`); this test pins the *semantics* it has
    /// to preserve.
    #[test]
    fn render_panic_payload_truncates_multibyte_at_a_char_boundary() {
        let payload = "é".repeat(1_000_000); // 2 MB, 1M chars
        let rendered = render_panic_payload(&payload);

        assert_eq!(
            rendered.chars().count(),
            201,
            "expected MAX_CHARS plus the ellipsis, split on a char boundary"
        );
        assert!(
            rendered.ends_with('…'),
            "truncation must be marked: {rendered:?}"
        );
        assert!(
            rendered.starts_with(&"é".repeat(200)),
            "the kept prefix must be verbatim, not re-encoded"
        );
    }
}
