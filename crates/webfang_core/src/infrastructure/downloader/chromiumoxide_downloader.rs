//! Chromiumoxide (CDP) downloader — real Chrome DevTools Protocol integration.
//!
//! Spawns a headless Chrome instance per fetch, injects cookies from the
//! [`CookieBridge`], navigates to the target URL, and extracts the rendered
//! HTML. The browser is closed after each fetch to bound memory usage.
//!
//! When the `chromium` feature is disabled, the module compiles as a stub
//! that returns an explicit "not enabled" error from every `fetch` call.

#[cfg(feature = "chromium")]
use {
    chromiumoxide::browser::HeadlessMode,
    chromiumoxide::cdp::browser_protocol::network::{
        CookieParam, EnableParams as NetworkEnableParams, EventLoadingFailed, EventLoadingFinished,
        EventRequestWillBeSent, SetCookiesParams,
    },
    chromiumoxide::{listeners::EventStream, Browser, BrowserConfig, Page},
    futures::StreamExt,
    tokio::time::{timeout, Duration},
};

#[cfg(all(test, feature = "chromium"))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use futures::future::BoxFuture;
use tokio::sync::RwLock;
use url::Url;

#[cfg(feature = "chromium")]
use super::chrome_profile::ChromeProfileDir;
use super::{DownloadError, Downloader, FetchedPage};
#[cfg(feature = "chromium")]
use crate::domain::cookie_bridge::domain_matches;
use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::post_load_wait::PostLoadWait;

/// Memory budget for one Chrome tab (~200 MB).
#[cfg(feature = "chromium")]
const CHROMIUMOXIDE_MEMORY_COST: usize = 200_000_000;

/// Timeout for browser navigation (page.goto).
#[cfg(feature = "chromium")]
const NAV_TIMEOUT: Duration = Duration::from_secs(30);

/// Timeout for content extraction (page.content).
#[cfg(feature = "chromium")]
const CONTENT_TIMEOUT: Duration = Duration::from_secs(10);

/// RAII guard over the CDP pump task spawned per `fetch` (#1129).
///
/// Dropping a `tokio` [`JoinHandle`](tokio::task::JoinHandle) only detaches
/// the task, so every early `?` return (`new_page`, cookies, navigation or
/// content timeout) — and every external cancellation of the `fetch` future
/// at an `.await` point — would orphan the `while handler.next().await`
/// pump. Holding this guard across all awaits makes cleanup total: `Drop`
/// aborts the pump, and the still-owned `Browser` local drops right after,
/// whose `Drop` kills the Chrome child via `kill_on_drop` (background reap).
/// The happy path consumes the guard through [`HandlerGuard::join`] after a
/// graceful close, so neither the abort nor the kill-on-drop fires there.
#[cfg(feature = "chromium")]
struct HandlerGuard(Option<tokio::task::JoinHandle<()>>);

#[cfg(feature = "chromium")]
impl HandlerGuard {
    /// Happy-path shutdown: wait for the pump to drain after `Browser::close`.
    /// Taking the handle out of the `Option` skips the `Drop` abort.
    async fn join(mut self) {
        if let Some(job) = self.0.take() {
            let _ = job.await;
        }
    }
}

#[cfg(feature = "chromium")]
impl Drop for HandlerGuard {
    fn drop(&mut self) {
        if let Some(job) = self.0.as_ref() {
            job.abort();
        }
    }
}

/// CDP downloader that spawns a headless Chrome instance per fetch.
///
/// Note: Resource gating is handled by [`super::hybrid_router::HybridRouter`].
/// This downloader does NOT own a `ResourceGovernor` — the router checks
/// resources before invoking this layer.
pub struct ChromiumoxideDownloader {
    #[cfg(feature = "chromium")]
    cookie_bridge: Arc<RwLock<CookieBridge>>,
    /// Post-load settlement mode (F-52-b, #1277).
    #[cfg(feature = "chromium")]
    post_load_wait: PostLoadWait,
    /// Fetch ceiling bounding the idle wait (F-52-b).
    #[cfg(feature = "chromium")]
    timeout_secs: u64,
    /// Gate-certified Chrome binary (F-52-c, #1278). `Some` pins the launch
    /// via `chrome_executable`; `None` keeps auto-detection.
    #[cfg(feature = "chromium")]
    chrome_binary: Option<PathBuf>,
}

/// Wall-clock ms since `started`, saturating (never wraps).
#[cfg(feature = "chromium")]
fn waited_ms_since(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Outcome of the post-load settle stage (F-52-b) — infallible by
/// contract: the wait never fails the fetch, it only reports.
#[cfg(feature = "chromium")]
struct SettleOutcome {
    /// Wait mode that ran (`idle`, `<ms>`, `none`).
    mode: &'static str,
    /// Wall-clock time spent settling, in ms (0 when skipped).
    waited_ms: u64,
    /// Idle mode only: whether network-idle was reached before the ceiling.
    idle_reached: Option<bool>,
}

/// The three subscribed CDP network streams tracked as one unit.
#[cfg(feature = "chromium")]
struct IdleStreams {
    will_be_sent: EventStream<EventRequestWillBeSent>,
    loading_finished: EventStream<EventLoadingFinished>,
    loading_failed: EventStream<EventLoadingFailed>,
}

impl ChromiumoxideDownloader {
    #[cfg(feature = "chromium")]
    pub(crate) fn new(
        cookie_bridge: Arc<RwLock<CookieBridge>>,
        post_load_wait: PostLoadWait,
        timeout_secs: u64,
        chrome_binary: Option<PathBuf>,
    ) -> Self {
        Self {
            cookie_bridge,
            post_load_wait,
            timeout_secs,
            chrome_binary,
        }
    }

    /// The configured settle mode (seam for propagation tests).
    #[cfg(all(test, feature = "chromium"))]
    pub(crate) fn post_load_wait(&self) -> PostLoadWait {
        self.post_load_wait
    }

    /// The configured Chrome binary, if the preflight gate resolved one.
    ///
    /// Seam for the F-52-c propagation test (mirrors the Layer 2
    /// `.binary()` accessor, #787): proves the configured path reaches the
    /// launcher without spawning a browser.
    #[cfg(all(test, feature = "chromium"))]
    pub(crate) fn chrome_binary(&self) -> Option<&Path> {
        self.chrome_binary.as_deref()
    }

    #[cfg(not(feature = "chromium"))]
    pub(crate) fn new(
        _cookie_bridge: Arc<RwLock<CookieBridge>>,
        _post_load_wait: PostLoadWait,
        _timeout_secs: u64,
        _chrome_binary: Option<PathBuf>,
    ) -> Self {
        Self {}
    }

    /// Post-load settlement between navigation and capture (F-52-b).
    ///
    /// Infallible: every failure path (CDP subscription, ceiling expiry)
    /// logs and proceeds with the current DOM — wait errors are
    /// observability events, never [`DownloadError`] variants.
    ///
    /// Cancellation: the fetch future owns this sleep/loop, so dropping
    /// the future (shutdown, timeout) aborts the wait promptly — no token
    /// is captured in the layer (tokens are per-run lifecycle, see
    /// `DownloaderSpec` docs). This is the documented deviation from the
    /// design sketch's `select!`-over-token: same shutdown promptness,
    /// no lifecycle captured in infrastructure state.
    #[cfg(feature = "chromium")]
    async fn settle_after_load(&self, url: &Url, streams: Option<IdleStreams>) -> SettleOutcome {
        let started = std::time::Instant::now();
        match self.post_load_wait {
            PostLoadWait::None => SettleOutcome {
                mode: "none",
                waited_ms: 0,
                idle_reached: None,
            },
            PostLoadWait::Fixed(ms) => {
                tokio::time::sleep(Duration::from_millis(u64::from(ms))).await;
                SettleOutcome {
                    mode: "fixed",
                    waited_ms: waited_ms_since(started),
                    idle_reached: None,
                }
            },
            PostLoadWait::Idle => self.settle_idle(url, started, streams).await,
        }
    }

    /// Network-idle settle over pre-armed streams: proceed once no request
    /// has been in flight for [`PostLoadWait::IDLE_WINDOW`], bounded by
    /// `timeout_secs`. `None` streams (arming failed) degrade to immediate
    /// capture with a WARN.
    #[cfg(feature = "chromium")]
    async fn settle_idle(
        &self,
        url: &Url,
        started: std::time::Instant,
        streams: Option<IdleStreams>,
    ) -> SettleOutcome {
        let finished = |idle_reached: bool| SettleOutcome {
            mode: "idle",
            waited_ms: waited_ms_since(started),
            idle_reached: Some(idle_reached),
        };
        let mut streams = match streams {
            Some(streams) => streams,
            None => {
                tracing::warn!(url = %url, "idle detection unavailable — degrading");
                return finished(false);
            },
        };
        let ceiling = Duration::from_secs(self.timeout_secs.max(1));
        match tokio::time::timeout(ceiling, Self::drain_until_quiet(&mut streams)).await {
            Ok(()) => finished(true),
            Err(_) => {
                tracing::warn!(url = %url, "network-idle not reached before ceiling");
                finished(false)
            },
        }
    }

    /// Subscribe to the CDP network lifecycle on the page, enabling Network
    /// tracking first. `None` (with WARN) on any failure — the caller
    /// degrades to immediate capture. Armed BEFORE navigation so requests
    /// fired during parse/load are observed from the start: a slow
    /// round-trip started pre-subscription would otherwise be invisible to
    /// the idle counter (F-52-b E2E dnet shape).
    #[cfg(feature = "chromium")]
    async fn subscribe_idle_streams(page: &Page, url: &Url) -> Option<IdleStreams> {
        // Best-effort enable: without Network tracking no events flow.
        if page.execute(NetworkEnableParams::default()).await.is_err() {
            tracing::warn!(url = %url, "idle detection unavailable — degrading");
            return None;
        }
        Some(IdleStreams {
            will_be_sent: Self::subscribe_one(page, url, "request-start").await?,
            loading_finished: Self::subscribe_one(page, url, "request-finish").await?,
            loading_failed: Self::subscribe_one(page, url, "request-failure").await?,
        })
    }

    /// Real navigation HTTP status for a just-loaded page (#1311).
    /// Fails closed: no recorded response (or no numeric status) is an
    /// honest `Internal` error, never a synthetic 200.
    #[cfg(feature = "chromium")]
    async fn navigation_status(page: &Page, url: &Url) -> Result<u16, DownloadError> {
        let navigation = timeout(NAV_TIMEOUT, page.wait_for_navigation_response())
            .await
            .map_err(|_| {
                DownloadError::Internal(format!("navigation to {url} produced no HTTP response"))
            })?
            .map_err(|e| DownloadError::Internal(e.to_string()))?;
        navigation
            .as_ref()
            .and_then(|request| request.response.as_ref())
            .map(|response| response.status)
            .and_then(|code| u16::try_from(code).ok())
            .ok_or_else(|| {
                DownloadError::Internal(format!("navigation to {url} produced no HTTP status"))
            })
    }

    /// Subscribe to one CDP network event kind. `None` (with WARN) on
    /// failure — the caller degrades to immediate capture. `what` names
    /// the subscription for the log line only.
    #[cfg(feature = "chromium")]
    async fn subscribe_one<T>(page: &Page, url: &Url, what: &'static str) -> Option<EventStream<T>>
    where
        T: chromiumoxide::cdp::IntoEventKind,
    {
        match page.event_listener::<T>().await {
            Ok(stream) => Some(stream),
            Err(_) => {
                tracing::warn!(url = %url, what, "idle detection unavailable — degrading");
                None
            },
        }
    }

    #[cfg(feature = "chromium")]
    async fn drain_until_quiet(streams: &mut IdleStreams) {
        let mut in_flight: u64 = 0;
        loop {
            tokio::select! {
                event = streams.will_be_sent.next() => {
                    if event.is_some() {
                        in_flight = in_flight.saturating_add(1);
                    }
                },
                    event = streams.loading_finished.next() => {
                        if event.is_some() {
                            // Orphan completion: started before subscription.
                            // Observed activity — restart the quiet window.
                            if in_flight == 0 {
                                continue;
                            }
                            in_flight -= 1;
                        }
                    },
                    event = streams.loading_failed.next() => {
                        if event.is_some() {
                            // Orphan completion: see above.
                            if in_flight == 0 {
                                continue;
                            }
                            in_flight -= 1;
                        }
                    },
                () = tokio::time::sleep(PostLoadWait::IDLE_WINDOW) => {
                    if in_flight == 0 {
                        break;
                    }
                },
            }
        }
    }
}

#[cfg(feature = "chromium")]
impl Downloader for ChromiumoxideDownloader {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(async move {
            // 1. Early URL scheme validation
            if !url.scheme().starts_with("http") {
                return Err(DownloadError::InvalidUrl(format!(
                    "unsupported scheme: {}",
                    url.scheme()
                )));
            }

            // 2. Unique user-data-dir per launch to avoid Chrome singleton lock.
            let profile = ChromeProfileDir::new().map_err(|e| {
                DownloadError::Internal(format!("failed to create Chrome profile dir: {e}"))
            })?;

            // 3. Browser config with sandbox bypass for CI/Docker.
            // F-52-c (#1278): when the preflight gate certified a binary,
            // launch exactly it instead of chromiumoxide auto-detection.
            let mut config_builder = BrowserConfig::builder()
                .headless_mode(HeadlessMode::True)
                .no_sandbox()
                .user_data_dir(profile.path());
            if let Some(path) = &self.chrome_binary {
                tracing::debug!(chrome_binary = %path.display(), "using gate-certified chrome binary");
                config_builder = config_builder.chrome_executable(path);
            }
            let config = config_builder
                .build()
                // LCOV_EXCL_LINE defensive: browser-config-build — static builder flags cannot fail at runtime
                .map_err(DownloadError::Internal)?;

            let (mut browser, mut handler) = Browser::launch(config)
                .await
                .map_err(|e| DownloadError::Internal(format!("Chrome launch failed: {e}")))?;

            // 3. Process CDP messages in isolated task to prevent hangs.
            // `HandlerGuard` aborts the pump on every non-`join` exit
            // (error `?` or async cancellation); dropping the `JoinHandle`
            // alone would only detach it (#1129).
            let handler_job = tokio::spawn(async move { while handler.next().await.is_some() {} });
            let guard = HandlerGuard(Some(handler_job));

            // 4. Inject cookies from L1 cookie bridge, filtered by domain.
            // #1119: `tokio::sync::RwLock` — the read lock is acquired with
            // `.await` (a contended bridge yields the worker instead of
            // parking an executor thread) and cannot be poisoned, so there
            // is no panic path inside the fetch future. The guard is scoped
            // and dropped before the next `.await`.
            let current_domain = url.host_str().unwrap_or("");
            let cdp_cookies: Vec<CookieParam> = {
                let bridge = self.cookie_bridge.read().await;
                bridge
                    .to_cdp_cookies()
                    .into_iter()
                    .filter(|c| domain_matches(current_domain, c.domain()))
                    .map(|c| {
                        let mut param = CookieParam::new(c.name(), c.value());
                        param.domain = Some(c.domain().to_string());
                        param.path = Some(c.path().to_string());
                        param.secure = Some(c.secure());
                        param.http_only = Some(c.http_only());
                        param
                    })
                    .collect()
            };

            let page = browser
                .new_page("about:blank")
                .await
                .map_err(|e| DownloadError::Internal(e.to_string()))?;

            if !cdp_cookies.is_empty() {
                page.execute(SetCookiesParams::new(cdp_cookies))
                    .await
                    .map_err(|e| DownloadError::Internal(e.to_string()))?;
            }

            // 5a. Arm idle tracking BEFORE navigation (F-52-b): only the
            // Idle mode subscribes; other modes skip the overhead entirely.
            let idle_streams = if matches!(self.post_load_wait, PostLoadWait::Idle) {
                Self::subscribe_idle_streams(&page, url).await
            } else {
                None
            };
            // 5. Navigate with timeout, capturing the REAL navigation HTTP
            // status (#1311). `goto` resolves after load but carries no
            // status, so the waiter below reads the main frame's recorded
            // navigation request. Queried AFTER `goto`: the handler answers
            // immediately when the frame is already loaded, so there is no
            // missed-navigation race; a missing record fails closed below.
            timeout(NAV_TIMEOUT, page.goto(url.as_str()))
                .await
                .map_err(|_| DownloadError::Timeout(NAV_TIMEOUT.as_secs()))?
                .map_err(|e| DownloadError::Internal(e.to_string()))?;
            let status = Self::navigation_status(&page, url).await?;

            // 5.5 Post-load settle (F-52-b, #1277): bounded wait so
            // post-load hydration lands before capture. Best-effort: never
            // fails the fetch, only reports via tracing.
            let settle = self.settle_after_load(url, idle_streams).await;
            tracing::debug!(
                post_load_wait_mode = settle.mode,
                post_load_wait_ms = settle.waited_ms,
                idle_reached = settle.idle_reached,
                url = %url,
                "post-load settle complete"
            );
            // 6. Extract rendered DOM with timeout
            let html = timeout(CONTENT_TIMEOUT, page.content())
                .await
                .map_err(|_| DownloadError::Timeout(CONTENT_TIMEOUT.as_secs()))?
                .map_err(|e| DownloadError::Internal(e.to_string()))?;

            // 7. Deterministic shutdown: graceful close, then drain the pump
            // through the guard (skips the `Drop` abort). Early `?` exits
            // and cancellation never reach here — the guard aborts the pump
            // and `Browser::drop` kills the child via `kill_on_drop`.
            browser.close().await.ok();
            guard.join().await;

            Ok(FetchedPage {
                url: url.clone(),
                html,
                status,
                headers: std::collections::HashMap::new(),
                cookies: Vec::new(),
            })
        })
    }

    fn supports_interactions(&self) -> bool {
        true
    }

    fn memory_cost(&self) -> usize {
        CHROMIUMOXIDE_MEMORY_COST
    }
}

#[cfg(not(feature = "chromium"))]
impl Downloader for ChromiumoxideDownloader {
    fn fetch<'a>(&'a self, _url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(async move {
            Err(DownloadError::FeatureGated(
                "Chromiumoxide not enabled (compile with --features chromium)".to_string(),
            ))
        })
    }

    fn supports_interactions(&self) -> bool {
        false
    }

    fn memory_cost(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "chromium"))]
    #[tokio::test]
    async fn test_chromiumoxide_returns_stub_error() {
        let dl = ChromiumoxideDownloader::new(
            Arc::new(RwLock::new(CookieBridge::new())),
            PostLoadWait::None,
            30,
            None,
        );
        let url: Url = "https://example.com".parse().unwrap();
        let err = dl.fetch(&url).await.unwrap_err();
        assert!(
            matches!(err, DownloadError::FeatureGated(ref msg) if msg.contains("not enabled")),
            "expected FeatureGated stub error (PermanentFatal, no retry), got: {err}"
        );
    }

    #[test]
    #[cfg(feature = "chromium")]
    fn test_chromiumoxide_metadata() {
        let dl = ChromiumoxideDownloader::new(
            Arc::new(RwLock::new(CookieBridge::new())),
            PostLoadWait::None,
            30,
            None,
        );
        assert!(dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 200_000_000);
    }

    /// F-52-b E2E (#1277): settle contract against loopback fixtures.
    ///
    /// Two hydration mechanisms, two waits: timer-driven mutation (d200)
    /// is captured by a sufficient fixed floor; network-driven hydration
    /// (dnet, delayed /api/data round-trip) is captured by network-idle.
    /// `None` captures immediately (historical behavior) — asserted for
    /// success only, never for content, because CDP latency past the load
    /// event is inherently racy. Skips gracefully where no Chrome binary
    /// exists (bare CI).
    #[cfg(feature = "chromium")]
    #[tokio::test]
    async fn settle_modes_capture_delayed_mutation() {
        if !chrome_present_for_e2e() {
            eprintln!("skipping settle E2E: no Chrome binary on PATH");
            return;
        }
        let server = wiremock::MockServer::start().await;
        macro_rules! mount_page {
            ($name:literal, $fixture:literal) => {
                wiremock::Mock::given(wiremock::matchers::method("GET"))
                    .and(wiremock::matchers::path($name))
                    .respond_with(
                        wiremock::ResponseTemplate::new(200)
                            .set_body_raw(include_str!($fixture), "text/html"),
                    )
                    .mount(&server)
                    .await;
            };
        }
        mount_page!(
            "d200",
            "../../../../../evidence/mode-d/fixtures/f52/d200.html"
        );
        mount_page!(
            "d1200",
            "../../../../../evidence/mode-d/fixtures/f52/d1200.html"
        );
        mount_page!(
            "dnet",
            "../../../../../evidence/mode-d/fixtures/f52/dnet.html"
        );
        // The API round-trip responds after a server-side delay, keeping
        // the request in flight long past any CDP round-trip latency.
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("api/data"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_raw(
                        "DNETM RENDERED MARKER content fetched over the network after load.",
                        "text/plain",
                    )
                    .set_delay(std::time::Duration::from_millis(800)),
            )
            .mount(&server)
            .await;
        let server_uri = server.uri();
        async fn fetch_page(server_uri: &str, mode: PostLoadWait, path: &str) -> FetchedPage {
            let dl = ChromiumoxideDownloader::new(
                Arc::new(RwLock::new(CookieBridge::new())),
                mode,
                30,
                None,
            );
            let url: Url = format!("{server_uri}{path}").parse().expect("wiremock uri");
            dl.fetch(&url).await.expect("loopback fetch must succeed")
        }
        // Mutation signal: the placeholder is gone from the live DOM once
        // hydration replaces the slot. (The marker *string* also lives
        // inside the page's own <script>, so asserting on it would pass
        // vacuously — the placeholder is the honest signal.)
        let placeholder = "PLACEHOLDER_INNER_HTML_STATIC_TEXT";
        // Timer mechanism: a sufficient fixed floor captures; the floor
        // does not stretch to later mutations.
        let page = fetch_page(&server_uri, PostLoadWait::Fixed(400), "/d200").await;
        assert!(
            !page.html.contains(placeholder),
            "fixed(400) must capture the 200ms timer mutation"
        );
        let page = fetch_page(&server_uri, PostLoadWait::Fixed(400), "/d1200").await;
        assert!(
            page.html.contains(placeholder),
            "fixed(400) must not capture the 1200ms timer mutation"
        );
        // Network mechanism: idle observes the round-trip; immediate
        // capture races it (800ms server delay makes the outcome stable).
        let page = fetch_page(&server_uri, PostLoadWait::Idle, "/dnet").await;
        assert!(
            !page.html.contains(placeholder),
            "idle must capture the network-driven hydration"
        );
        let page = fetch_page(&server_uri, PostLoadWait::None, "/dnet").await;
        assert!(
            page.html.contains(placeholder),
            "none must capture before the 800ms network round-trip lands"
        );
        // Historical immediacy: `None` succeeds; content intentionally
        // unasserted (CDP latency past load is racy by nature).
        fetch_page(&server_uri, PostLoadWait::None, "/d200").await;
    }

    /// Best-effort Chrome presence probe so the E2E skips (rather than
    /// fails) on machines without a browser. Test-only; the production
    /// gate owns real resolution (preflight + #1278).
    #[cfg(feature = "chromium")]
    fn chrome_present_for_e2e() -> bool {
        [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
        ]
        .iter()
        .any(|binary| {
            std::process::Command::new(binary)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    /// The guard must abort the pump task when the fetch scope exits without
    /// `join` (error `?` or async cancellation, #1129). Proved by observing
    /// the spawned future's `Drop`: an aborted task drops its future.
    #[tokio::test]
    #[cfg(feature = "chromium")]
    async fn handler_guard_aborts_pump_on_early_exit() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };

        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let future_dropped = Arc::new(AtomicBool::new(false));
        let flag = DropFlag(future_dropped.clone());
        let job = tokio::spawn(async move {
            let _flag = flag;
            std::future::pending::<()>().await;
        });
        drop(HandlerGuard(Some(job)));

        timeout(Duration::from_secs(5), async {
            while !future_dropped.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborted pump task must drop its future promptly");
    }

    #[test]
    #[cfg(not(feature = "chromium"))]
    fn test_chromiumoxide_metadata_stub() {
        let dl = ChromiumoxideDownloader::new(
            Arc::new(RwLock::new(CookieBridge::new())),
            PostLoadWait::None,
            30,
            None,
        );
        assert!(!dl.supports_interactions());
        assert_eq!(dl.memory_cost(), 0);
    }
}
