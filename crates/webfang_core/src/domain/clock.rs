//! Clock ports — Injected time abstractions for deterministic testing.
//!
//! Following Hexagonal Architecture: the domain layer defines time ports,
//! production code uses real clocks, tests inject mock clocks.
//!
//! # Port Types
//!
//! - [`Clock`] — For `Instant`-based timing (rate limiters, session pools)
//! - [`UtcClock`] — For `DateTime<Utc>`-based timestamps (credentials, exports)

use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Clock port for `Instant`-based timing operations.
///
/// Used by components that need monotonic time measurements:
/// rate limiters, session pools, retry backoff.
pub trait Clock: Send + Sync {
    /// Returns the current monotonic time.
    fn now(&self) -> Instant;
}

/// Clock port for `DateTime<Utc>`-based timestamp operations.
///
/// Used by components that need wall-clock timestamps:
/// credential expiry, export timestamps, audit logs.
pub trait UtcClock: Send + Sync {
    /// Returns the current UTC timestamp.
    fn now(&self) -> DateTime<Utc>;
}

// ============================================================================
// Production Implementations
// ============================================================================

/// System clock using real `Instant::now()`.
pub struct SystemClock;

impl Default for SystemClock {
    fn default() -> Self {
        Self
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// System clock using real `Utc::now()`.
pub struct SystemUtcClock;

impl UtcClock for SystemUtcClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

// ============================================================================
// Test Doubles
// ============================================================================

/// Mock clock for deterministic `Instant`-based testing.
///
/// Uses `Arc<Mutex<Instant>>` internally so the clock can be shared
/// between the pool (via `Arc<dyn Clock>`) and the test (via `MockClock`).
/// Advancing through the `MockClock` is visible to the pool.
///
/// # Example
///
/// ```rust
/// use std::time::{Duration, Instant};
/// use webfang_core::domain::clock::{Clock, MockClock};
///
/// let clock = MockClock::new(Instant::now());
/// let t0 = clock.now();
///
/// // Advance time by 100ms
/// clock.advance(Duration::from_millis(100));
/// assert_eq!(clock.now(), t0 + Duration::from_millis(100));
/// ```
pub struct MockClock {
    now: Arc<Mutex<Instant>>,
}

impl MockClock {
    /// Create a mock clock starting at the given instant.
    pub fn new(now: Instant) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    /// Get an `Arc<dyn Clock>` handle sharing this clock's time.
    ///
    /// Advancing this `MockClock` will be visible through the returned handle.
    pub fn handle(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.now) as Arc<dyn Clock>
    }

    /// Advance the clock by the given duration.
    ///
    /// `MockClock` is a test utility; mutex poisoning indicates a test bug,
    /// not a recoverable runtime error.
    #[allow(clippy::expect_used)]
    pub fn advance(&self, duration: std::time::Duration) {
        *self.now.lock().expect("mock clock poisoned") += duration;
    }

    /// Set the clock to a specific instant.
    ///
    /// `MockClock` is a test utility; mutex poisoning indicates a test bug,
    /// not a recoverable runtime error.
    #[allow(clippy::expect_used)]
    pub fn set_now(&self, now: Instant) {
        *self.now.lock().expect("mock clock poisoned") = now;
    }
}

impl Clock for MockClock {
    // `MockClock` is a test utility; mutex poisoning indicates a test bug,
    // not a recoverable runtime error.
    #[allow(clippy::expect_used)]
    fn now(&self) -> Instant {
        *self.now.lock().expect("mock clock poisoned")
    }
}

impl Clock for Mutex<Instant> {
    // `MockClock` is a test utility; mutex poisoning indicates a test bug,
    // not a recoverable runtime error.
    #[allow(clippy::expect_used)]
    fn now(&self) -> Instant {
        *self.lock().expect("mock clock poisoned")
    }
}

/// Mock UTC clock for deterministic `DateTime<Utc>`-based testing.
///
/// # Example
///
/// ```rust
/// use chrono::{Duration, Utc};
/// use webfang_core::domain::clock::{UtcClock, MockUtcClock};
///
/// let t0 = Utc::now();
/// let mut clock = MockUtcClock::new(t0);
/// assert_eq!(clock.now(), t0);
///
/// clock.advance(Duration::hours(1));
/// assert_eq!(clock.now(), t0 + Duration::hours(1));
/// ```
pub struct MockUtcClock {
    now: DateTime<Utc>,
}

impl MockUtcClock {
    /// Create a mock clock starting at the given timestamp.
    pub fn new(now: DateTime<Utc>) -> Self {
        Self { now }
    }

    /// Advance the clock by the given duration.
    pub fn advance(&mut self, duration: chrono::Duration) {
        self.now += duration;
    }

    /// Set the clock to a specific timestamp.
    pub fn set_now(&mut self, now: DateTime<Utc>) {
        self.now = now;
    }
}

impl UtcClock for MockUtcClock {
    fn now(&self) -> DateTime<Utc> {
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_system_clock_returns_instant() {
        let clock = SystemClock;
        let before = Instant::now();
        let result = clock.now();
        let after = Instant::now();
        assert!(result >= before && result <= after);
    }

    #[test]
    fn test_system_utc_clock_returns_now() {
        let clock = SystemUtcClock;
        let before = Utc::now();
        let result = clock.now();
        let after = Utc::now();
        assert!(result >= before && result <= after);
    }

    #[test]
    fn test_mock_clock_returns_set_value() {
        let t0 = Instant::now();
        let clock = MockClock::new(t0);
        assert_eq!(clock.now(), t0);
    }

    #[test]
    fn test_mock_clock_advance() {
        let t0 = Instant::now();
        let clock = MockClock::new(t0);
        clock.advance(Duration::from_millis(500));
        assert_eq!(clock.now(), t0 + Duration::from_millis(500));
    }

    #[test]
    fn test_mock_clock_set_now() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(10);
        let clock = MockClock::new(t0);
        clock.set_now(t1);
        assert_eq!(clock.now(), t1);
    }

    #[test]
    fn test_mock_utc_clock_returns_set_value() {
        let t0 = Utc::now();
        let clock = MockUtcClock::new(t0);
        assert_eq!(clock.now(), t0);
    }

    #[test]
    fn test_mock_utc_clock_advance() {
        let t0 = Utc::now();
        let mut clock = MockUtcClock::new(t0);
        clock.advance(chrono::Duration::hours(2));
        assert_eq!(clock.now(), t0 + chrono::Duration::hours(2));
    }

    #[test]
    fn test_mock_utc_clock_set_now() {
        let t0 = Utc::now();
        let t1 = t0 + chrono::Duration::days(30);
        let mut clock = MockUtcClock::new(t0);
        clock.set_now(t1);
        assert_eq!(clock.now(), t1);
    }

    #[test]
    fn test_mock_clock_multiple_advances() {
        let t0 = Instant::now();
        let clock = MockClock::new(t0);
        clock.advance(Duration::from_millis(100));
        clock.advance(Duration::from_millis(200));
        clock.advance(Duration::from_millis(300));
        assert_eq!(clock.now(), t0 + Duration::from_millis(600));
    }
}
