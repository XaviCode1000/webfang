//! Rate Limiter module — Token Bucket implementation using governor
//!
//! Extracts the rate limiting logic out of the crawler orchestration modules
//! to allow for independent testing.
//!
//! # Design Decisions
//!
//! - Uses `governor` crate with Token Bucket algorithm
//! - Thread-safe via Arc (shares across async tasks)
//! - Configurable delay and burst parameters
//! - No Mutex needed - governor handles internal synchronization
//!
//! # Waits are observable (#1610, OBS-P1-2 / OBS-H2 / OBS-M1)
//!
//! Pacing is invisible by construction: a wait produces no event, so a slow
//! crawl looks identical to a slow network. The [`PacingContext`] passed to
//! the `*_observed` wait variants carries the identifying fields (scope, URL,
//! correlation identity), and the wait emits one structured
//! `rate limit wait` event with its measured cost. Measured BEFORE the wait is
//! emitted, not after the fetch, so the number answers "how long did pacing
//! hold this request back", never "how long did the page take".

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

#[cfg(miri)]
use governor::clock::MonotonicClock;
#[cfg(not(miri))]
use governor::clock::QuantaClock;
use governor::{
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovernorLimiter,
};
use tokio_util::sync::CancellationToken;

use crate::error::ScraperError;

/// Clock used by the governor rate limiters.
///
/// Production builds use governor's `QuantaClock` (quanta TSC probing, ~100ns
/// resolution). Miri cannot execute quanta's `raw-cpuid` inline assembly
/// (`unsupported operation: inline assembly is not supported`, #1514), so
/// Miri builds swap in governor's `MonotonicClock` — pure `std::time::Instant`,
/// no FFI, no inline asm. Spacing semantics are identical; only the time-source
/// resolution differs, and only under `cfg(miri)`.
#[cfg(miri)]
/// Miri build: pure `std::time::Instant` clock (no quanta probing).
pub type GovernorClock = MonotonicClock;
#[cfg(not(miri))]
/// Production build: quanta TSC-backed clock (~100ns resolution).
pub type GovernorClock = QuantaClock;

/// Middleware matching `GovernorClock`.
///
/// governor's default middleware parameter is anchored to
/// `<DefaultClock as Clock>::Instant` (= `QuantaInstant` with the `quanta`
/// feature), so under Miri — where `GovernorClock` is `MonotonicClock` with
/// `Instant = std::time::Instant` — the default would mismatch the 3-generic
/// limiter type. Mirror the default explicitly per cfg (#1514).
#[cfg(miri)]
/// Miri build: middleware over `std::time::Instant`.
pub type GovernorMiddleware = governor::middleware::NoOpMiddleware<std::time::Instant>;
#[cfg(not(miri))]
/// Production build: governor's default (middleware over `QuantaInstant`).
pub type GovernorMiddleware = governor::middleware::NoOpMiddleware;

/// Type alias for the rate limiter - allows swapping implementations
pub type CrawlRateLimiter =
    GovernorLimiter<NotKeyed, InMemoryState, GovernorClock, GovernorMiddleware>;

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Delay between requests in milliseconds
    pub delay_ms: u64,
    /// Maximum concurrent requests (burst)
    pub concurrency: u32,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            delay_ms: 100,
            concurrency: 5,
        }
    }
}

impl RateLimiterConfig {
    /// Create new configuration
    pub fn new(delay_ms: u64, concurrency: u32) -> Self {
        Self {
            delay_ms,
            concurrency,
        }
    }
}

/// A rate-limit wait was cancelled before a permit was granted (#509).
///
/// Control signal, not an operational failure: the engine's cancellation
/// token fired while the task waited for a token-bucket permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("rate limit wait cancelled by engine shutdown")]
pub struct RateLimitCancelled;

/// `operation` field of every event a pacing wait emits (#1610).
///
/// Dotted `resource.action` shape, shared with the session-cap and rate-limit
/// events the MCP server already publishes (#1611) so one `jq` can select
/// every admission/pacing decision across the two transports.
pub const RATE_LIMIT_WAIT_OPERATION: &str = "http.rate_limit.wait";

/// Identifying context for one observed pacing wait (#1610).
///
/// Passed by the caller because the limiter is shared: it cannot know which
/// URL it is pacing, nor which operation is waiting. `url` and `correlation`
/// are optional — a wait is still measured without them, and when they are
/// absent the corresponding fields are OMITTED from the event rather than
/// emitted empty (the #698 rule: the presence of a key implies a real
/// identity).
#[derive(Debug, Clone, Copy)]
pub struct PacingContext<'a> {
    /// Which pacing site is waiting (`"crawl_discovery"`, `"cli_scrape"`,
    /// `"batch_scrape"`, `"http_client"`) — the dimension that answers
    /// "which path is being throttled".
    pub scope: &'static str,
    /// URL being paced, when the call site knows it.
    pub url: Option<&'a str>,
    /// Identity to attribute the wait to. Pacing happens BEFORE the page span
    /// in the crawl path (OBS-H2), so without this the wait would belong to no
    /// span at all and could not be joined back to its page.
    pub correlation: Option<&'a crate::domain::CorrelationId>,
}

impl<'a> PacingContext<'a> {
    /// A pacing context for a site with no known target or identity.
    #[must_use]
    pub fn bare(scope: &'static str) -> Self {
        Self {
            scope,
            url: None,
            correlation: None,
        }
    }

    /// Attach the URL being paced.
    #[must_use]
    pub fn with_url(mut self, url: &'a str) -> Self {
        self.url = Some(url);
        self
    }

    /// Attach the operation identity the wait belongs to.
    #[must_use]
    pub fn with_correlation(mut self, correlation: &'a crate::domain::CorrelationId) -> Self {
        self.correlation = Some(correlation);
        self
    }
}

/// Emit the one structured event that makes a pacing wait measurable (#1610).
///
/// Public because not every pacing site holds a [`SharedRateLimiter`]: the
/// HTTP client carries a bare `governor::RateLimiter` (per-session quota), so
/// it measures its own wait and reports it through the same helper. One
/// emitter, one event shape, two limiter types.
///
/// DEBUG level on purpose: a paced fetch emits this on EVERY page, and the
/// console filter honours the operator's verbosity. The `--trace-file` layer
/// always runs at TRACE (`init_logging_dual`), so the event is in the JSONL
/// whether or not anyone asked for `-vv` — the trace file is the observability
/// surface, stderr is the human one.
pub fn record_pacing_wait(ctx: &PacingContext<'_>, waited: Duration, outcome: &'static str) {
    let waited_ms = waited.as_millis() as u64;
    let url = ctx.url.unwrap_or("unknown");
    match ctx.correlation {
        Some(correlation) => {
            tracing::debug!(
                operation = RATE_LIMIT_WAIT_OPERATION,
                scope = ctx.scope,
                url,
                correlation_id = %correlation,
                trace_id = %correlation.trace_id(),
                waited_ms,
                outcome,
                "rate limit wait"
            );
        },
        None => {
            tracing::debug!(
                operation = RATE_LIMIT_WAIT_OPERATION,
                scope = ctx.scope,
                url,
                waited_ms,
                outcome,
                "rate limit wait"
            );
        },
    }
}

/// Shared rate limiter for crawl operations
#[derive(Clone)]
pub struct SharedRateLimiter(Arc<CrawlRateLimiter>);

impl SharedRateLimiter {
    /// Create a new shared rate limiter from config
    pub fn new(config: &RateLimiterConfig) -> Result<Self, ScraperError> {
        // A zero delay means "no delay between requests": governor rejects a
        // zero period (`Quota::with_period(Duration::ZERO)` is `None`), which
        // would otherwise surface as a config error and abort discovery.
        // Clamp to a 1ms floor so the bucket stays valid while imposing no
        // perceptible spacing (the burst grants `concurrency` immediate
        // permits).
        let period_ms = config.delay_ms.max(1);
        let quota = Quota::with_period(Duration::from_millis(period_ms))
            .ok_or_else(|| ScraperError::Config("Invalid period".into()))?;

        let quota = quota.allow_burst(
            NonZeroU32::new(config.concurrency)
                .ok_or_else(|| ScraperError::Config("Concurrency must be > 0".into()))?,
        );

        let limiter = GovernorLimiter::direct_with_clock(quota, &GovernorClock::default());
        Ok(Self(Arc::new(limiter)))
    }

    /// Wait until a permit is available
    pub async fn until_ready(&self) {
        self.0.until_ready().await;
    }

    /// Wait until a permit is available, abandoning the wait if `cancel`
    /// fires first (#509).
    ///
    /// Dropping the pending governor future consumes no token, so a
    /// cancelled caller leaves the bucket untouched for the next task.
    ///
    /// # Errors
    ///
    /// Returns [`RateLimitCancelled`] when the token fires before a permit
    /// is granted.
    pub async fn until_ready_or_cancel(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), RateLimitCancelled> {
        tokio::select! {
            () = self.0.until_ready() => Ok(()),
            () = cancel.cancelled() => Err(RateLimitCancelled),
        }
    }

    /// [`Self::until_ready`] with the wait measured and emitted (#1610).
    ///
    /// Returns the time actually spent waiting — `Duration::ZERO` when a
    /// burst permit was available immediately, which is the normal case and
    /// is still emitted so "no pacing at all" is a queryable observation
    /// rather than an absence.
    pub async fn until_ready_observed(&self, ctx: &PacingContext<'_>) -> Duration {
        let started = std::time::Instant::now();
        self.0.until_ready().await;
        let waited = started.elapsed();
        record_pacing_wait(ctx, waited, "granted");
        waited
    }

    /// [`Self::until_ready_or_cancel`] with the wait measured and emitted
    /// (#1610).
    ///
    /// A cancelled wait is a SKIP, not a failure (#509), but it is still a
    /// wait somebody paid for: it emits with `outcome = "cancelled"` and
    /// returns the time spent before the token fired.
    ///
    /// # Errors
    ///
    /// Returns [`RateLimitCancelled`] when the token fires before a permit
    /// is granted.
    pub async fn until_ready_or_cancel_observed(
        &self,
        ctx: &PacingContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Duration, RateLimitCancelled> {
        let started = std::time::Instant::now();
        let result = tokio::select! {
            () = self.0.until_ready() => Ok(()),
            () = cancel.cancelled() => Err(RateLimitCancelled),
        };
        let waited = started.elapsed();
        record_pacing_wait(
            ctx,
            waited,
            match result {
                Ok(()) => "granted",
                Err(_) => "cancelled",
            },
        );
        result.map(|()| waited)
    }
}

impl From<GovernorLimiter<NotKeyed, InMemoryState, GovernorClock, GovernorMiddleware>>
    for SharedRateLimiter
{
    fn from(
        limiter: GovernorLimiter<NotKeyed, InMemoryState, GovernorClock, GovernorMiddleware>,
    ) -> Self {
        Self(Arc::new(limiter))
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    /// `tracing` caches the per-callsite `Interest` process-wide: the first
    /// thread to reach the `rate limit wait` callsite WITHOUT a subscriber
    /// caches `Interest::never()` forever, which would silently turn these
    /// assertions into a test of nothing. Installing a global sink subscriber
    /// once makes every callsite register as always-interested; the per-test
    /// `FileTraceLayer` below still collects what it needs.
    static GLOBAL_SUBSCRIBER: std::sync::Once = std::sync::Once::new();

    fn ensure_global_subscriber() {
        GLOBAL_SUBSCRIBER.call_once(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::fmt()
                    .with_writer(std::io::sink)
                    .finish(),
            );
        });
    }

    /// Run `body` under a real `FileTraceLayer` and return the JSONL records.
    ///
    /// The production layer — not a mock — is the assertion surface: what this
    /// proves is that the event survives the same path `--trace-file` uses.
    /// The body drives its own runtime (governor needs a Tokio timer), so the
    /// whole future stays inside the `with_default` scope and every event it
    /// emits is captured.
    fn capture_trace<F, Fut>(body: F) -> Vec<serde_json::Value>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        ensure_global_subscriber();
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("trace.jsonl");
        let layer = crate::infrastructure::observability::FileTraceLayer::new(path.clone())
            .expect("trace layer");
        let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        tracing::dispatcher::with_default(&dispatch, || runtime.block_on(body()));
        std::fs::read_to_string(&path)
            .expect("trace file")
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
            .collect()
    }

    fn wait_events(records: &[serde_json::Value]) -> Vec<&serde_json::Value> {
        records
            .iter()
            .filter(|r| r["message"] == "rate limit wait")
            .collect()
    }

    /// #1610 (OBS-P1-2 / OBS-M1): a paced wait emits ONE structured event
    /// carrying the identifying fields and its measured cost.
    #[test]
    fn observed_wait_emits_measured_event() {
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(120, 1)).unwrap();
        let correlation = crate::domain::CorrelationId::new();

        let records = capture_trace(|| async {
            // Burn the burst so the next wait is a REAL, measurable one.
            limiter.until_ready().await;
            let ctx = PacingContext::bare("crawl_discovery")
                .with_url("https://example.com/page")
                .with_correlation(&correlation);
            let waited = limiter.until_ready_observed(&ctx).await;
            assert!(
                waited >= Duration::from_millis(100),
                "measured wait must reflect the real delay, got {waited:?}"
            );
        });

        let events = wait_events(&records);
        assert_eq!(events.len(), 1, "exactly one wait event");
        let fields = events[0]["fields"].as_object().expect("fields");
        assert_eq!(
            fields["operation"].as_str(),
            Some("http.rate_limit.wait"),
            "operation names the governed resource"
        );
        assert_eq!(fields["scope"].as_str(), Some("crawl_discovery"));
        assert_eq!(
            fields["url"].as_str(),
            Some("https://example.com/page"),
            "OBS-H2: the wait must name the URL it delayed"
        );
        assert_eq!(fields["outcome"].as_str(), Some("granted"));
        assert_eq!(
            fields["correlation_id"].as_str(),
            Some(correlation.to_traceparent().as_str()),
            "OBS-H2: the wait must carry the page identity"
        );
        assert!(
            fields["waited_ms"].as_u64().is_some_and(|ms| ms >= 100),
            "waited_ms must carry the measured cost, got {:?}",
            fields["waited_ms"]
        );
    }

    /// #1610: a burst permit costs nothing but is STILL emitted, so "this run
    /// was never paced" is an observation rather than an absence.
    #[test]
    fn observed_wait_emits_zero_cost_for_burst_permit() {
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(5_000, 4)).unwrap();
        let records = capture_trace(|| async {
            let ctx = PacingContext::bare("cli_scrape").with_url("https://example.com/a");
            assert!(
                limiter.until_ready_observed(&ctx).await < Duration::from_millis(1),
                "a burst permit must report no wait"
            );
        });

        let events = wait_events(&records);
        assert_eq!(events.len(), 1);
        let fields = events[0]["fields"].as_object().expect("fields");
        assert_eq!(fields["waited_ms"].as_u64(), Some(0));
        assert!(
            !fields.contains_key("correlation_id"),
            "a wait with no identity must OMIT the key, not report an empty one"
        );
        assert!(
            !fields.contains_key("trace_id"),
            "no trace_id without identity"
        );
    }

    /// #1610: a wait abandoned by shutdown still reports what it cost, with
    /// `outcome = "cancelled"` — a skip that stays invisible is how a run
    /// ends up "0 pages" for nobody's known reason (#509).
    #[test]
    fn observed_wait_reports_cancelled_outcome() {
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(60_000, 1)).unwrap();
        let cancel = CancellationToken::new();

        let records = capture_trace(|| async {
            limiter.until_ready().await; // burn the burst: the next wait blocks a minute
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.cancel();
            let ctx = PacingContext::bare("crawl_discovery").with_url("https://example.com/slow");
            let result = limiter.until_ready_or_cancel_observed(&ctx, &cancel).await;
            assert!(matches!(result, Err(RateLimitCancelled)));
        });

        let events = wait_events(&records);
        assert_eq!(events.len(), 1);
        let fields = events[0]["fields"].as_object().expect("fields");
        assert_eq!(fields["outcome"].as_str(), Some("cancelled"));
    }

    #[test]
    fn test_rate_limiter_config_default() {
        let config = RateLimiterConfig::new(100, 5);
        assert_eq!(config.delay_ms, 100);
        assert_eq!(config.concurrency, 5);
    }

    #[test]
    fn test_rate_limiter_config_default_values() {
        // Verifica valores por defecto
        let config = RateLimiterConfig::new(100, 5);
        assert_eq!(config.delay_ms, 100);
        assert_eq!(config.concurrency, 5);
    }

    // ============================================================================
    // Behavioral Rate Limiting Tests
    // ============================================================================

    #[tokio::test]
    async fn test_rate_limiter_until_ready_spreads_over_time() {
        // Test que N tasks concurrentes llamando until_ready() son espaciadas
        // Config: delay_ms=50ms, concurrency=1
        // 5 tasks → mínimo ~200ms de spread total
        // Mide elapsed y verifica >= (N-1) * delay

        let config = RateLimiterConfig::new(50, 1); // 50ms entre requests, burst=1
        let limiter = SharedRateLimiter::new(&config).unwrap();

        let num_tasks = 5;
        let start = std::time::Instant::now();

        let mut handles = Vec::new();
        for _ in 0..num_tasks {
            let limiter = limiter.clone();
            let handle = tokio::spawn(async move {
                limiter.until_ready().await;
            });
            handles.push(handle);
        }

        futures::future::join_all(handles).await;
        let elapsed = start.elapsed();

        // 5 tasks con delay de 50ms → mínimo ~200ms
        // Con algo de jitter, verificamos al menos 150ms (75% de teórico)
        let min_expected_ms = 150;
        assert!(
            elapsed.as_millis() >= min_expected_ms,
            "Tiempo transcurrido {}ms < {}ms mínimo — rate limiter no está espaciando",
            elapsed.as_millis(),
            min_expected_ms
        );
    }

    #[test]
    fn test_rate_limiter_burst_allows_parallel_requests() {
        // Deterministic burst proof. governor grants `concurrency` burst
        // permits immediately, without consuming real time. We drive a
        // `FakeRelativeClock`-backed limiter and assert the burst is exhausted by the
        // Nth non-blocking acquisition — no real-time upper bound, so it
        // cannot flake under CI load.
        use governor::clock::FakeRelativeClock;
        use governor::RateLimiter as GovernorLimiter;
        use std::num::NonZeroU32;
        use std::time::Duration;

        let clock = FakeRelativeClock::default();
        let quota = Quota::with_period(Duration::from_millis(100))
            .expect("valid period")
            .allow_burst(NonZeroU32::new(5).expect("valid burst"));
        let limiter = GovernorLimiter::direct_with_clock(quota, &clock);

        // All 5 burst permits are granted immediately and deterministically.
        for i in 0..5 {
            assert!(
                limiter.check().is_ok(),
                "burst permit {i} should be available without waiting"
            );
        }

        // The 6th acquisition must be rejected: the burst is exhausted and a
        // refill would require advancing the (still-zero) FakeClock.
        assert!(
            limiter.check().is_err(),
            "burst exhausted after 5 acquires — 6th must wait for refill"
        );
    }

    #[tokio::test]
    async fn test_rate_limiter_concurrent_backpressure() {
        // Test que 20 tasks concurrentes no colapsan — se encolan correctamente
        let config = RateLimiterConfig::new(10, 1); // 10ms, burst=1
        let limiter = SharedRateLimiter::new(&config).unwrap();

        let num_tasks = 20;
        let start = std::time::Instant::now();

        let mut handles = Vec::new();
        for _ in 0..num_tasks {
            let limiter = limiter.clone();
            let handle = tokio::spawn(async move {
                limiter.until_ready().await;
            });
            handles.push(handle);
        }

        futures::future::join_all(handles).await;
        let elapsed = start.elapsed();

        // 20 tasks × 10ms delay = 190ms mínimo
        // Verificamos que tomó al menos 100ms (rate limiting activo)
        assert!(
            elapsed.as_millis() >= 100,
            "20 tasks completaron en {}ms — rate limiting no está regulando",
            elapsed.as_millis()
        );
    }

    #[test]
    fn test_rate_limiter_config_zero_delay_is_valid_noop() {
        // delay_ms=0 → "sin delay": el bucket debe construirse (no error).
        // Se clampa a 1ms internamente para que governor sea válido.
        let config = RateLimiterConfig::new(0, 1);
        let result = SharedRateLimiter::new(&config);
        assert!(
            result.is_ok(),
            "delay_ms=0 debería ser válido (sin delay), no un error de configuración"
        );
    }

    #[test]
    fn test_rate_limiter_config_zero_concurrency_returns_error() {
        // concurrency=0 → debe retornar error, no panic
        let config = RateLimiterConfig::new(100, 0);
        let result = SharedRateLimiter::new(&config);
        assert!(result.is_err(), "concurrency=0 debería retornar error");
    }

    #[test]
    fn test_shared_rate_limiter_creation_success() {
        let config = RateLimiterConfig::new(50, 3);
        let limiter = SharedRateLimiter::new(&config);
        assert!(limiter.is_ok(), "valid config should create limiter");
    }

    // ============================================================================
    // M5: Deterministic Rate Limiting Tests
    //
    // NOTE: governor uses QuantaClock (real time), so its `until_ready()`
    // enforces delays against the wall clock, not tokio's mockable clock.
    // `tokio::time::pause()` therefore freezes our measurement clock while
    // governor may decide the wait has already elapsed in real time, making
    // these assertions report 0ns and flake under load. We measure with
    // `std::time::Instant` instead: governor guarantees it will not return
    // before the real delay has elapsed, so this is deterministic.
    //
    // IMPORTANT: MockClock from domain::clock is designed for CONTROLLING
    // time in tests (advance/set_now), not for measuring real elapsed time.
    // Since governor uses real time internally, we use std::time::Instant
    // for measurement. MockClock is used below for testing components
    // that accept Clock as a dependency parameter (not governor).
    // ============================================================================

    use crate::domain::clock::{Clock, MockClock};

    #[tokio::test]
    async fn test_rate_limiting_precision() {
        let config = RateLimiterConfig::new(500, 1);
        let limiter = SharedRateLimiter::new(&config).unwrap();

        limiter.until_ready().await;
        let start = std::time::Instant::now();

        limiter.until_ready().await;

        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(500),
            "Rate limiter should enforce 500ms delay, got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn test_rate_limiting_burst_protection() {
        let config = RateLimiterConfig::new(100, 2); // 100ms delay, burst=2
        let limiter = SharedRateLimiter::new(&config).unwrap();

        // First 2 should succeed immediately (burst=2)
        limiter.until_ready().await;
        limiter.until_ready().await;

        // Third should be delayed
        let start = std::time::Instant::now();
        limiter.until_ready().await;
        let elapsed = start.elapsed();

        assert!(
            elapsed >= std::time::Duration::from_millis(100),
            "Third request should be delayed by 100ms, got {elapsed:?}"
        );
    }

    /// Sustained-pressure (no-starvation) proof for #517 hallazgo 5.
    ///
    /// With burst = `concurrency`, N concurrent waiters where N >> burst must
    /// ALL complete, and the tail must respect the configured period — the
    /// burst only front-loads the first `concurrency` permits, it must not
    /// inflate the sustained rate.
    #[tokio::test]
    async fn test_rate_limiting_sustained_pressure_no_starvation() {
        let config = RateLimiterConfig::new(100, 4); // 100ms period, burst=4
        let limiter = SharedRateLimiter::new(&config).unwrap();

        let num_tasks = 16; // 4× the burst — sustained pressure
        let start = std::time::Instant::now();

        let mut handles = Vec::new();
        for _ in 0..num_tasks {
            let limiter = limiter.clone();
            let handle = tokio::spawn(async move {
                limiter.until_ready().await;
            });
            handles.push(handle);
        }

        futures::future::join_all(handles).await;
        let elapsed = start.elapsed();

        // All 16 completed (no starvation — the join_all above would not
        // return otherwise). The last permit is gated by the 12th refill
        // after the burst: (16 - 4) × 100ms = 1200ms. Assert ≥ 1000ms so a
        // single lost refill does not flake, but the sustained rate clearly
        // did not collapse to burst-speed.
        assert!(
            elapsed >= std::time::Duration::from_millis(1000),
            "sustained rate collapsed: 16 tasks with period=100ms/burst=4 took {elapsed:?}"
        );
    }

    // ============================================================================
    // MockClock unit tests (testing the Clock port itself)
    //
    // These verify MockClock works correctly as a test double.
    // They demonstrate the pattern for components that accept &dyn Clock.
    // ============================================================================

    #[test]
    fn test_mock_clock_advance_tracks_elapsed() {
        let t0 = std::time::Instant::now();
        let clock = MockClock::new(t0);

        // Advance by 100ms
        clock.advance(std::time::Duration::from_millis(100));
        assert_eq!(
            clock.now().duration_since(t0),
            std::time::Duration::from_millis(100)
        );

        // Advance by another 200ms (total 300ms)
        clock.advance(std::time::Duration::from_millis(200));
        assert_eq!(
            clock.now().duration_since(t0),
            std::time::Duration::from_millis(300)
        );
    }

    #[test]
    fn test_mock_clock_set_now_overrides() {
        let t0 = std::time::Instant::now();
        let clock = MockClock::new(t0);

        clock.advance(std::time::Duration::from_secs(10));
        assert_eq!(
            clock.now().duration_since(t0),
            std::time::Duration::from_secs(10)
        );

        // Set to a specific point
        let target = t0 + std::time::Duration::from_secs(5);
        clock.set_now(target);
        assert_eq!(
            clock.now().duration_since(t0),
            std::time::Duration::from_secs(5)
        );
    }

    #[test]
    fn test_mock_clock_duration_between_two_points() {
        let t0 = std::time::Instant::now();
        let clock = MockClock::new(t0);

        let start = clock.now();
        clock.advance(std::time::Duration::from_millis(250));
        let end = clock.now();

        let elapsed = end.duration_since(start);
        assert_eq!(elapsed, std::time::Duration::from_millis(250));
    }

    // ============================================================================
    // Configuration validation tests
    // ============================================================================

    #[test]
    fn test_rate_limiter_config_various_valid_values() {
        assert!(SharedRateLimiter::new(&RateLimiterConfig::new(1, 1)).is_ok());
        assert!(SharedRateLimiter::new(&RateLimiterConfig::new(1000, 100)).is_ok());
        assert!(SharedRateLimiter::new(&RateLimiterConfig::new(50, 10)).is_ok());
    }

    #[test]
    fn test_rate_limiter_config_extreme_burst() {
        let config = RateLimiterConfig::new(100, 1000);
        assert!(SharedRateLimiter::new(&config).is_ok());
    }

    // ============================================================================
    // Cancellation tests (#509)
    // ============================================================================

    #[tokio::test]
    async fn until_ready_or_cancel_grants_permit_from_available_burst() {
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(100, 5)).unwrap();
        let cancel = CancellationToken::new();

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            limiter.until_ready_or_cancel(&cancel),
        )
        .await;

        assert!(matches!(result, Ok(Ok(()))));
    }

    #[tokio::test]
    async fn until_ready_or_cancel_returns_cancelled_while_waiting() {
        // 60s period with burst 2: exhaust the burst, then the third wait
        // would block for ~60s without cancellation.
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(60_000, 2)).unwrap();
        let cancel = CancellationToken::new();
        limiter.until_ready().await;
        limiter.until_ready().await;

        let waiter = {
            let limiter = limiter.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { limiter.until_ready_or_cancel(&cancel).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), waiter).await;
        assert!(matches!(result, Ok(Ok(Err(RateLimitCancelled)))));
    }

    #[tokio::test]
    async fn until_ready_or_cancel_bucket_survives_cancelled_wait() {
        // 500ms period, burst 1: consume the token, block a waiter, cancel it
        // before the first refill (~100ms < 500ms).
        let limiter = SharedRateLimiter::new(&RateLimiterConfig::new(500, 1)).unwrap();
        let cancel = CancellationToken::new();
        limiter.until_ready().await; // consume the single burst token

        let start = tokio::time::Instant::now();
        let waiter = {
            let limiter = limiter.clone();
            let cancel = cancel.clone();
            tokio::spawn(async move { limiter.until_ready_or_cancel(&cancel).await })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), waiter).await,
            Ok(Ok(Err(RateLimitCancelled)))
        ));

        // The next wait must obtain the first period's refill (~500ms from
        // start) — if the cancelled wait had consumed a token it would take
        // ~1000ms. Asserting well below the leak case proves no state leaked.
        let next = tokio::time::timeout(
            Duration::from_secs(2),
            limiter.until_ready_or_cancel(&CancellationToken::new()),
        )
        .await;
        assert!(matches!(next, Ok(Ok(()))));
        assert!(
            start.elapsed() < Duration::from_millis(900),
            "cancelled wait leaked a token: next refill took {:?}",
            start.elapsed()
        );
    }
}
