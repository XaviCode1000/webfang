//! Rate Limiter module — Token Bucket implementation using governor
//!
//! Extracts the rate limiting logic from crawler_service.rs to allow
//! for independent testing.
//!
//! # Design Decisions
//!
//! - Uses `governor` crate with Token Bucket algorithm
//! - Thread-safe via Arc (shares across async tasks)
//! - Configurable delay and burst parameters
//! - No Mutex needed - governor handles internal synchronization

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{
    clock::QuantaClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter as GovernorLimiter,
};
use tokio_util::sync::CancellationToken;

use crate::error::ScraperError;

/// Type alias for the rate limiter - allows swapping implementations
pub type CrawlRateLimiter = GovernorLimiter<NotKeyed, InMemoryState, QuantaClock>;

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

/// Shared rate limiter for crawl operations
#[derive(Clone)]
pub struct SharedRateLimiter(Arc<CrawlRateLimiter>);

impl SharedRateLimiter {
    /// Create a new shared rate limiter from config
    pub fn new(config: &RateLimiterConfig) -> Result<Self, ScraperError> {
        let quota = Quota::with_period(Duration::from_millis(config.delay_ms))
            .ok_or_else(|| ScraperError::Config("Invalid period".into()))?;

        let quota = quota.allow_burst(
            NonZeroU32::new(config.concurrency)
                .ok_or_else(|| ScraperError::Config("Concurrency must be > 0".into()))?,
        );

        let limiter = GovernorLimiter::direct(quota);
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
}

impl From<GovernorLimiter<NotKeyed, InMemoryState, QuantaClock>> for SharedRateLimiter {
    fn from(limiter: GovernorLimiter<NotKeyed, InMemoryState, QuantaClock>) -> Self {
        Self(Arc::new(limiter))
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

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
    #[ignore = "timing-sensitive: run with cargo test -- --ignored"]
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

    #[tokio::test]
    #[ignore = "timing-sensitive: run with cargo test -- --ignored"]
    async fn test_rate_limiter_burst_allows_parallel_requests() {
        // Test que burst de N requests ocurren en paralelo
        // Config: delay_ms=100ms, concurrency=5
        // 5 tasks simultáneas → todas deben pasar rápido (dentro del burst)
        use tokio::time::Instant;

        let config = RateLimiterConfig::new(100, 5); // 100ms delay, burst=5
        let limiter = SharedRateLimiter::new(&config).unwrap();

        let num_tasks = 5;
        let start = Instant::now();

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

        // 5 tasks con burst=5 → todas deberían pasar casi instantáneo (< 50ms)
        assert!(
            elapsed.as_millis() < 50,
            "Tiempo {}ms > 50ms — burst no está funcionando",
            elapsed.as_millis()
        );
    }

    #[tokio::test]
    #[ignore = "timing-sensitive: run with cargo test -- --ignored"]
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
    fn test_rate_limiter_config_zero_delay_returns_error() {
        // delay_ms=0 → debe retornar error, no panic
        let config = RateLimiterConfig::new(0, 1);
        let result = SharedRateLimiter::new(&config);
        assert!(result.is_err(), "delay_ms=0 debería retornar error");
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
