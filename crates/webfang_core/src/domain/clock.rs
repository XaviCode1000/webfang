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
/// # Example
///
/// ```rust
/// use std::time::{Duration, Instant};
/// use webfang::domain::clock::{Clock, MockClock};
///
/// let mut clock = MockClock::new(Instant::now());
/// let t0 = clock.now();
///
/// // Advance time by 100ms
/// clock.set_now(t0 + Duration::from_millis(100));
/// assert_eq!(clock.now(), t0 + Duration::from_millis(100));
/// ```
pub struct MockClock {
    now: Instant,
}

impl MockClock {
    /// Create a mock clock starting at the given instant.
    pub fn new(now: Instant) -> Self {
        Self { now }
    }

    /// Advance the clock by the given duration.
    pub fn advance(&mut self, duration: std::time::Duration) {
        self.now += duration;
    }

    /// Set the clock to a specific instant.
    pub fn set_now(&mut self, now: Instant) {
        self.now = now;
    }
}

impl Clock for MockClock {
    fn now(&self) -> Instant {
        self.now
    }
}

/// Mock UTC clock for deterministic `DateTime<Utc>`-based testing.
///
/// # Example
///
/// ```rust
/// use chrono::{Duration, Utc};
/// use webfang::domain::clock::{UtcClock, MockUtcClock};
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
        let mut clock = MockClock::new(t0);
        clock.advance(Duration::from_millis(500));
        assert_eq!(clock.now(), t0 + Duration::from_millis(500));
    }

    #[test]
    fn test_mock_clock_set_now() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_secs(10);
        let mut clock = MockClock::new(t0);
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
        let mut clock = MockClock::new(t0);
        clock.advance(Duration::from_millis(100));
        clock.advance(Duration::from_millis(200));
        clock.advance(Duration::from_millis(300));
        assert_eq!(clock.now(), t0 + Duration::from_millis(600));
    }
}
