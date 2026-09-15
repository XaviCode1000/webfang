//! #1433: deterministic request-counting fixture + arrival-spacing harness.
//!
//! The RC-4 net: externally-observable guarantees (request counts,
//! inter-arrival spacing) pinned by CI instead of regressing unnoticed.
//!
//! [`start_fixture`] binds an ephemeral loopback listener and serves every
//! route the net needs from one task. Each arrival is logged with its
//! timestamp, method, path, and headers, so a test reads as "3 URLs,
//! 3 arrivals, `>= 0.75x` delay apart" — asserting on [`RequestLog`] rather
//! than internal state.
//!
//! Hermeticity: loopback only, no real network, per-test listener on an
//! ephemeral port. SSRF-sensitive routes rely on the spawned binary's child
//! env (the documented test-only hatches via `common::cli_harness::cmd`),
//! never on process-global state.
//!
//! Lock discipline: the log mutex is held for a push/clone only, never
//! across `.await`. Per-route counters are atomics.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Tolerance factor for spacing assertions: arrivals must land at least
/// `0.75x` the nominal delay apart (same convention as the
/// `SharedRateLimiter` unit tests — arrival jitter exists, early refills
/// do not).
pub const SPACING_TOLERANCE: f64 = 0.75;

/// One observed arrival: when it reached the fixture, what it asked for.
#[derive(Debug, Clone)]
pub struct LoggedRequest {
    /// Server-side arrival instant (recorded when the request head completes).
    pub at: Instant,
    /// Uppercase HTTP method (`GET`, ...).
    pub method: String,
    /// Origin-form path without the query string (`/forbidden-always`).
    pub path: String,
    /// Value of the `user-agent` header, if present.
    pub user_agent: Option<String>,
    /// All request headers as `(lowercase-name, value)` pairs.
    pub headers: Vec<(String, String)>,
}

/// Shared arrival log: cloneable handle over the fixture's request journal.
///
/// All snapshots are insertion-ordered by arrival; [`RequestLog::gaps`] and
/// [`RequestLog::span`] sort by instant first so concurrent arrivals cannot
/// reorder the spacing math.
#[derive(Debug, Clone, Default)]
pub struct RequestLog {
    inner: Arc<std::sync::Mutex<Vec<LoggedRequest>>>,
}

impl RequestLog {
    /// Snapshot every logged arrival in arrival order.
    pub fn requests(&self) -> Vec<LoggedRequest> {
        self.inner
            .lock()
            .expect("request log lock is never poisoned")
            .clone()
    }

    /// Arrival-ordered paths (`/p0`, `/p1`, ...).
    pub fn paths(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|entry| entry.path.clone())
            .collect()
    }

    /// Total arrival count.
    pub fn count(&self) -> usize {
        self.inner
            .lock()
            .expect("request log lock is never poisoned")
            .len()
    }

    /// Arrivals on exactly `path`.
    pub fn count_path(&self, path: &str) -> usize {
        self.inner
            .lock()
            .expect("request log lock is never poisoned")
            .iter()
            .filter(|entry| entry.path == path)
            .count()
    }

    /// Arrivals under a path prefix (`/p` covers `/p0..​/pN`).
    pub fn count_prefix(&self, prefix: &str) -> usize {
        self.inner
            .lock()
            .expect("request log lock is never poisoned")
            .iter()
            .filter(|entry| entry.path.starts_with(prefix))
            .count()
    }

    /// Arrival instants, oldest first.
    pub fn arrivals(&self) -> Vec<Instant> {
        let mut instants: Vec<Instant> = self.requests().iter().map(|entry| entry.at).collect();
        instants.sort_unstable();
        instants
    }

    /// Successive inter-arrival gaps, oldest first. Empty for `< 2` arrivals.
    pub fn gaps(&self) -> Vec<Duration> {
        self.arrivals()
            .windows(2)
            .map(|pair| pair[1].duration_since(pair[0]))
            .collect()
    }

    /// Oldest-to-newest arrival span. Zero for `< 2` arrivals.
    pub fn span(&self) -> Duration {
        let instants = self.arrivals();
        match (instants.first(), instants.last()) {
            (Some(first), Some(last)) => last.duration_since(*first),
            _ => Duration::ZERO,
        }
    }

    /// Arrival-ordered `user-agent` values (one entry per request).
    pub fn user_agents(&self) -> Vec<Option<String>> {
        self.requests()
            .iter()
            .map(|entry| entry.user_agent.clone())
            .collect()
    }
}

/// Start the fixture: bind an ephemeral loopback listener, serve it from a
/// dedicated OS thread with its own runtime, and return the base URL plus
/// the shared log.
///
/// The thread matters: `#[tokio::test]` defaults to a current-thread
/// runtime, and the test blocks that thread waiting on the spawned binary
/// — a `tokio::spawn`ed accept loop would starve and every connection
/// would time out. Owning the thread keeps arrivals flowing while the
/// test blocks on process exit.
///
/// The listener lives until the test process exits; each test gets its own
/// port, so parallel tests cannot observe each other.
pub async fn start_fixture() -> (String, RequestLog) {
    let state = Arc::new(FixtureState::new());
    // Bind with std (no runtime affinity): a tokio listener is tied to the
    // reactor of the runtime that created it, and polling it from this
    // thread's runtime stalls silently — zero bytes served, client timeouts.
    // The conversion happens inside the server thread below.
    let std_listener = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("fixture listener binds an ephemeral loopback port");
    // `TcpListener::from_std` requires a non-blocking socket.
    std_listener
        .set_nonblocking(true)
        .expect("fixture listener sets non-blocking mode");
    let base_url = format!(
        "http://{}",
        std_listener
            .local_addr()
            .expect("fixture listener has a local address")
    );
    let log = state.log.clone();
    std::thread::Builder::new()
        .name("webfang-fixture-server".to_string())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("fixture runtime builds");
            runtime.block_on(async move {
                let listener = TcpListener::from_std(std_listener)
                    .expect("fixture listener converts to tokio");
                accept_loop(listener, state).await;
            });
        })
        .expect("fixture server thread spawns");
    (base_url, log)
}

// ---------------------------------------------------------------------------
// Server internals
// ---------------------------------------------------------------------------

/// Per-route mutable state. Counters are atomics (no lock); the
/// default-UA route remembers the first-seen identity under a short lock.
struct FixtureState {
    log: RequestLog,
    limited_hits: AtomicUsize,
    flaky_hits: AtomicUsize,
    default_ua_first: std::sync::Mutex<Option<String>>,
}

impl FixtureState {
    fn new() -> Self {
        Self {
            log: RequestLog::default(),
            limited_hits: AtomicUsize::new(0),
            flaky_hits: AtomicUsize::new(0),
            default_ua_first: std::sync::Mutex::new(None),
        }
    }
}

/// Substantive article body so the export pipeline keeps every page (an
/// empty body risks extractor-drop noise in the arrival count, which is
/// itself an assertion in the pacing test).
fn article_body() -> Vec<u8> {
    "<html><head><title>Fixture probe</title></head><body><main><article>\
     <h1>Fixture probe page</h1>\
     <p>The harbor ledger records every tide that ever reached the stone quay, \
     and the clerks copy each entry twice so no storm can erase the account of \
     what the sea returned.</p>\
     </article></main></body></html>"
        .as_bytes()
        .to_vec()
}

/// Page carrying duplicate links to `/ok` (dedup behavior probe).
fn dupes_body() -> Vec<u8> {
    "<html><head><title>Dupes</title></head><body><main><article>\
     <h1>Duplicate links probe</h1>\
     <p>The harbor ledger records every tide that ever reached the stone quay, \
     and the clerks copy each entry twice so no storm can erase the account.</p>\
     <p><a href=\"/ok\">mirror</a> <a href=\"/ok\">mirror again</a> \
     <a href=\"/ok\">mirror a third time</a></p>\
     </article></main></body></html>"
        .as_bytes()
        .to_vec()
}

/// Cyclic-graph nodes: each links the other.
fn cycle_body(other: &str) -> Vec<u8> {
    format!(
        "<html><head><title>Cycle</title></head><body><main><article>\
         <h1>Cycle probe</h1>\
         <p>The harbor ledger records every tide that ever reached the stone quay, \
         and the clerks copy each entry twice so no storm can erase the account.</p>\
         <p><a href=\"{other}\">the other node</a></p>\
         </article></main></body></html>"
    )
    .into_bytes()
}

/// Deliberately malformed HTML (unclosed tags, stray markup).
fn malformed_body() -> Vec<u8> {
    "<html><head><title>Broken fixture<div><p>The harbor ledger records every \
     tide that ever reached the stone quay, and the clerks copy each entry \
     twice so no storm can erase <b>the account of what the sea <i>returned."
        .as_bytes()
        .to_vec()
}

/// Sitemap listing the fixture's own pages (absolute locs via Host header).
fn sitemap_body(host: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
         <url><loc>http://{host}/ok</loc></url>\n\
         <url><loc>http://{host}/dupes</loc></url>\n\
         </urlset>"
    )
    .into_bytes()
}

/// Sitemap index pointing at this fixture's sitemap.
fn sitemap_index_body(host: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
         <sitemap><loc>http://{host}/sitemap.xml</loc></sitemap>\n\
         </sitemapindex>"
    )
    .into_bytes()
}

/// Large body (~240 KiB of filler paragraphs) for bounded-read probes.
fn large_body() -> Vec<u8> {
    static CACHED: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    CACHED
        .get_or_init(|| {
            let mut body = article_body();
            let filler = "<p>Filler paragraph padding the large-body probe so bounded \
                 readers observe a multi-hundred-kilobyte page.</p>";
            body.extend(filler.as_bytes().repeat(4000));
            body
        })
        .clone()
}

/// Gzip-encoded repetitive HTML (`Content-Encoding: gzip`).
fn gzip_bomb_body() -> &'static [u8] {
    // gzip of 64x "<html><body><article><p>The harbor ledger ... quay.</p>...
    // (~7 KiB at ~40x ratio). Precomputed so the fixture needs no new deps.
    static BYTES: &[u8] = &[
        31, 139, 8, 8, 163, 188, 168, 106, 0, 3, 119, 102, 49, 52, 51, 51, 95, 98, 111, 109, 98,
        46, 116, 120, 116, 0, 237, 205, 193, 13, 194, 48, 16, 68, 209, 59, 85, 108, 5, 184, 129,
        149, 171, 160, 1, 199, 94, 225, 72, 6, 135, 141, 65, 74, 247, 68, 65, 164, 138, 127, 156,
        25, 105, 158, 214, 241, 104, 81, 167, 94, 182, 168, 201, 199, 156, 155, 69, 93, 226, 173,
        154, 212, 228, 83, 119, 105, 86, 238, 230, 226, 150, 187, 151, 85, 236, 99, 190, 201, 152,
        139, 201, 168, 105, 28, 121, 31, 83, 174, 86, 246, 198, 100, 29, 253, 105, 242, 122, 167,
        237, 170, 97, 137, 26, 206, 219, 240, 99, 194, 97, 94, 20, 26, 26, 26, 26, 26, 26, 26, 26,
        26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 250,
        79, 127, 1, 61, 223, 48, 180, 192, 30, 0, 0,
    ];
    BYTES
}

/// Minimal response: status, extra headers, body.
struct FixtureResponse {
    status: u16,
    reason: &'static str,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl FixtureResponse {
    fn ok(body: Vec<u8>, content_type: &str) -> Self {
        Self {
            status: 200,
            reason: "OK",
            headers: vec![("content-type".to_string(), content_type.to_string())],
            body,
        }
    }

    fn html(body: Vec<u8>) -> Self {
        Self::ok(body, "text/html; charset=utf-8")
    }

    fn status_only(status: u16, reason: &'static str, body: &str) -> Self {
        Self {
            status,
            reason,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body: body.as_bytes().to_vec(),
        }
    }

    fn redirect(status: u16, reason: &'static str, location: &str) -> Self {
        Self {
            status,
            reason,
            headers: vec![("location".to_string(), location.to_string())],
            body: Vec::new(),
        }
    }
}

/// Stateful routes (counters / first-seen identity). Returns `Some` when the
/// path is stateful, `None` to fall through to the static table.
fn stateful_route(
    path: &str,
    user_agent: Option<&str>,
    state: &FixtureState,
) -> Option<FixtureResponse> {
    if path == "/limited" {
        let hit = state.limited_hits.fetch_add(1, Ordering::SeqCst);
        if hit == 0 {
            return Some(FixtureResponse {
                status: 429,
                reason: "Too Many Requests",
                headers: vec![("retry-after".to_string(), "0".to_string())],
                body: "rate limited".as_bytes().to_vec(),
            });
        }
        return Some(FixtureResponse::html(article_body()));
    }
    if path == "/flaky" {
        let hit = state.flaky_hits.fetch_add(1, Ordering::SeqCst);
        if hit < 2 {
            return Some(FixtureResponse::status_only(
                500,
                "Internal Server Error",
                "flaky failure",
            ));
        }
        return Some(FixtureResponse::html(article_body()));
    }
    if path == "/forbidden-default-ua" {
        // 403 for the default UA only: the first-seen identity is refused,
        // any rotated identity is served. The test pins the observable
        // (two arrivals, distinct UAs, success) without hardcoding UA strings.
        let current = user_agent.unwrap_or("").to_string();
        let mut first = state
            .default_ua_first
            .lock()
            .expect("default-UA slot is never poisoned");
        let refused = match first.as_ref() {
            None => {
                *first = Some(current);
                true
            },
            Some(seen) => *seen == current,
        };
        if refused {
            return Some(FixtureResponse::status_only(403, "Forbidden", "forbidden"));
        }
        return Some(FixtureResponse::html(article_body()));
    }
    None
}

/// Static route table. `host` feeds the absolute sitemap locs.
fn static_route(path: &str, host: &str) -> FixtureResponse {
    if path == "/redirect" {
        return FixtureResponse::redirect(302, "Found", "/ok");
    }
    if path == "/redirect301" {
        return FixtureResponse::redirect(301, "Moved Permanently", "/ok");
    }
    if path == "/redir-loopback" {
        return FixtureResponse::redirect(302, "Found", "http://127.0.0.2:9/forbidden");
    }
    if path == "/forbidden-always" {
        return FixtureResponse::status_only(403, "Forbidden", "forbidden");
    }
    if path == "/boom-always" {
        return FixtureResponse::status_only(500, "Internal Server Error", "boom");
    }
    if path == "/slow" {
        // Handled by the caller (sleep before responding); unreachable here.
        return FixtureResponse::html(article_body());
    }
    if path == "/large" {
        return FixtureResponse::html(large_body());
    }
    if path == "/gzip-bomb" {
        return FixtureResponse {
            status: 200,
            reason: "OK",
            headers: vec![
                ("content-type".to_string(), "text/html".to_string()),
                ("content-encoding".to_string(), "gzip".to_string()),
            ],
            body: gzip_bomb_body().to_vec(),
        };
    }
    if path == "/dupes" {
        return FixtureResponse::html(dupes_body());
    }
    if path == "/cycle-a" {
        return FixtureResponse::html(cycle_body("/cycle-b"));
    }
    if path == "/cycle-b" {
        return FixtureResponse::html(cycle_body("/cycle-a"));
    }
    if path == "/malformed" {
        return FixtureResponse::html(malformed_body());
    }
    if path == "/sitemap.xml" {
        return FixtureResponse::ok(sitemap_body(host), "application/xml");
    }
    if path == "/sitemap-index.xml" {
        return FixtureResponse::ok(sitemap_index_body(host), "application/xml");
    }
    if path == "/robots.txt" {
        return FixtureResponse::ok(
            "User-agent: *\nAllow: /\n".as_bytes().to_vec(),
            "text/plain",
        );
    }
    if path == "/" || path == "/ok" || path.starts_with("/p") {
        return FixtureResponse::html(article_body());
    }
    FixtureResponse::status_only(404, "Not Found", "no such fixture route")
}

/// Accept loop: one spawned task per connection, detached from the caller.
async fn accept_loop(listener: TcpListener, state: Arc<FixtureState>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let state = state.clone();
        tokio::spawn(async move {
            serve_connection(stream, &state).await;
        });
    }
}

/// Serve a single connection: read one request head, log the arrival,
/// dispatch, respond, close. No log lock is held across `.await`.
async fn serve_connection(mut stream: TcpStream, state: &FixtureState) {
    let Some((method, path, headers)) = read_request_head(&mut stream).await else {
        return;
    };
    let user_agent = headers
        .iter()
        .find(|(name, _)| name == "user-agent")
        .map(|(_, value)| value.clone());
    // The arrival instant: recorded after the head completes, before dispatch.
    let arrival = LoggedRequest {
        at: Instant::now(),
        method: method.clone(),
        path: path.clone(),
        user_agent: user_agent.clone(),
        headers: headers.clone(),
    };
    {
        state
            .log
            .inner
            .lock()
            .expect("request log lock is never poisoned")
            .push(arrival);
    }

    if method != "GET" {
        write_response(
            &mut stream,
            &FixtureResponse::status_only(405, "Method Not Allowed", "fixture serves GET only"),
        )
        .await;
        return;
    }
    // The slow route sleeps before responding (timeout probe); the lock is
    // long released by now, so the sleep blocks nothing but this task.
    if path == "/slow" {
        tokio::time::sleep(Duration::from_secs(30)).await;
        write_response(&mut stream, &FixtureResponse::html(article_body())).await;
        return;
    }
    let host = headers
        .iter()
        .find(|(name, _)| name == "host")
        .map(|(_, value)| value.as_str())
        .unwrap_or("127.0.0.1");
    let response = stateful_route(&path, user_agent.as_deref(), state)
        .unwrap_or_else(|| static_route(&path, host));
    write_response(&mut stream, &response).await;
}

/// Read until the end of the request head (`\r\n\r\n`, 64 KiB cap).
/// Returns `(METHOD, path-without-query, headers)` with lowercase names.
async fn read_request_head(
    stream: &mut TcpStream,
) -> Option<(String, String, Vec<(String, String)>)> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..n]);
        if raw.len() > 65536 {
            return None;
        }
        if raw.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&raw);
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_uppercase();
    let target = parts.next().unwrap_or("/");
    let path = target.split(['?', '#']).next().unwrap_or("/").to_string();
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_lowercase(), value.trim().to_string()));
        }
    }
    Some((method, path, headers))
}

/// Serialize one response with `Connection: close` and flush it.
async fn write_response(stream: &mut TcpStream, response: &FixtureResponse) {
    let mut out = format!(
        "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n",
        response.status,
        response.reason,
        response.body.len()
    );
    for (name, value) in &response.headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(&response.body);
    let _ = stream.write_all(&bytes).await;
}
