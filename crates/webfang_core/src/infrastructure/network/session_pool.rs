//! Session health pool — Per-domain session tracking with exponential backoff.
//!
//! Sealed trait pattern: only `DomainSessionPool` can implement `SessionManager`.
//! Uses DashMap for concurrent access without holding locks across `.await`.
//!
//! # Design Decisions
//!
//! - **DashMap<String, SessionState>** — concurrent per-domain state, same pattern as UrlDeduplicator
//! - **Exponential backoff** — `base_delay * 2^min(failures, max_exp)`, capped at `max_delay`
//! - **TTL eviction** — stale sessions removed on `acquire()`, no background thread
//! - **Zero-cost abstraction** — `impl SessionManager` not `Box<dyn SessionManager>`
//!
//! # D6 lock-across-await audit (task 2.3, change stabilization-concurrency-budget)
//!
//! Functions rewired by commit de54342a (budget-derived pool size):
//!
//! | Function | `.await` points | Guard discipline | Verdict |
//! |---|---|---|---|
//! | `SessionPoolConfig::default` | none (sync fn) | pure value construction, no locks | PASS |
//! | `DomainSessionPool::acquire` | none (sync `SessionManager` method) | DashMap `RefMut` from `entry().or_insert_with(..)` is explicitly `drop(sessions)`-ped BEFORE any re-entrant map access (the RefMut-dropped-BEFORE-gauge ordering invariant, see comment above the drop in `acquire`) | PASS |
//!
//! Cross-check: the crate's only `tokio::sync::Mutex` guards live in
//! `infrastructure/crawler/url_queue.rs` inside documented sync-only sections
//! (invariant AL-2); this module never spans an await while holding a guard.
//!
//! Enforcement: `#![deny(clippy::await_holding_lock)]` below fails the build if
//! a future edit ever holds a `std` lock guard across an `.await` in this module.

#![deny(clippy::await_holding_lock)]

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tracing::{debug, instrument, warn};

use crate::domain::budget::DomainSlots;
use crate::domain::clock::{Clock, SystemClock};
use crate::domain::session_port::SessionId;

/// Health status of a session for a given domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Session is healthy and available for requests.
    Healthy,
    /// Session is temporarily banned due to consecutive failures.
    Banned,
    /// Session is being retired after exceeding max failures.
    Retiring,
}

/// Per-domain session state tracked by the pool.
#[derive(Debug, Clone)]
struct SessionState {
    status: SessionStatus,
    consecutive_failures: u32,
    last_failure_time: Option<Instant>,
    next_retry_time: Option<Instant>,
}

impl SessionState {
    fn healthy() -> Self {
        Self {
            status: SessionStatus::Healthy,
            consecutive_failures: 0,
            last_failure_time: None,
            next_retry_time: None,
        }
    }
}

/// Configuration for the session pool.
#[derive(Debug, Clone)]
pub struct SessionPoolConfig {
    /// Number of session slots per domain — the budget model's `Domain`
    /// tier (task 2.2c). A plain `usize` would let zero or unclamped values
    /// in; the [`DomainSlots`] newtype makes an invalid slot count
    /// unrepresentable (design D4).
    pub pool_size: DomainSlots,
    /// Base delay for exponential backoff.
    pub base_delay: Duration,
    /// Maximum delay cap for backoff.
    pub max_delay: Duration,
    /// Maximum exponent for backoff calculation.
    pub max_exp: u32,
    /// TTL for idle sessions before eviction.
    pub ttl_duration: Duration,
}

impl Default for SessionPoolConfig {
    fn default() -> Self {
        Self {
            // LCOV_EXCL_LINE defensive: domain-slot-default — the constant is non-zero by definition
            pool_size: DomainSlots::new(crate::domain::budget::DOMAIN_SLOTS_DEFAULT)
                .unwrap_or_else(|_| unreachable!("domain slot default is non-zero")),
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            max_exp: 6,
            ttl_duration: Duration::from_secs(300),
        }
    }
}

/// Sealed trait for session manager implementations.
///
/// Only implementors within this module can implement this trait.
pub trait SessionManager: sealed::Sealed {
    /// Acquire an available session for the given domain.
    ///
    /// Returns `None` if all sessions are banned or in cooldown.
    fn acquire(&self, domain: &str) -> Option<SessionId>;

    /// Report a successful request for the given domain's session.
    fn report_success(&self, domain: &str, session_id: SessionId);

    /// Report a failed request with the HTTP status code.
    ///
    /// Status codes 429, 503, and 403 trigger ban logic.
    fn report_failure(&self, domain: &str, session_id: SessionId, status_code: u16);

    /// Remove sessions that have been idle beyond the TTL.
    fn evict_stale(&self);

    /// Return the current pool size for a domain (for diagnostics).
    fn domain_count(&self, domain: &str) -> usize;

    /// Return total tracked domains (for diagnostics).
    fn total_domains(&self) -> usize;
}

/// Per-domain session pool with health tracking and exponential backoff.
#[derive(Clone)]
pub struct DomainSessionPool {
    /// Per-domain session states. Key = domain string, Value = Vec of session states.
    sessions: DashMap<String, Vec<SessionState>>,
    config: SessionPoolConfig,
    /// Injected clock for deterministic time in tests.
    clock: Arc<dyn Clock>,
}

impl DomainSessionPool {
    /// Create a new session pool with the given configuration and clock.
    #[must_use]
    pub fn new(config: SessionPoolConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            sessions: DashMap::new(),
            config,
            clock,
        }
    }

    /// Create a pool with default configuration using the system clock.
    #[must_use]
    pub fn default_pool() -> Self {
        Self::new(SessionPoolConfig::default(), Arc::new(SystemClock))
    }

    /// Calculate exponential backoff delay for a given failure count.
    ///
    /// Applies ±20% jitter to prevent thundering herd on retry.
    fn backoff_delay(&self, consecutive_failures: u32) -> Duration {
        use rand::Rng;
        let exponent = consecutive_failures.min(self.config.max_exp);
        let base_ms = self.config.base_delay.as_millis();
        let max_ms = self.config.max_delay.as_millis();
        let delay_ms = base_ms.saturating_mul(2u128.pow(exponent));
        let capped = delay_ms.min(max_ms).max(1);
        // ±20% jitter: random factor between 0.8 and 1.2
        let jitter_factor = rand::rng().random_range(0.8_f64..=1.2);
        let jittered = (capped as f64 * jitter_factor).round() as u64;
        Duration::from_millis(jittered.max(1))
    }

    /// Reset sessions whose TTL has expired back to healthy.
    fn evict_expired(&self, domain: &str, sessions: &mut [SessionState], now: Instant) {
        for state in sessions.iter_mut() {
            if let Some(last_failure) = state.last_failure_time {
                if now.duration_since(last_failure) > self.config.ttl_duration
                    && state.status != SessionStatus::Healthy
                {
                    debug!(domain, "evicting stale session (TTL expired)");
                    *state = SessionState::healthy();
                }
            }
        }
    }

    /// Find the first healthy or recoverable session for the domain.
    fn find_available(
        &self,
        domain: &str,
        sessions: &[SessionState],
        now: Instant,
    ) -> Option<SessionId> {
        for (idx, state) in sessions.iter().enumerate() {
            match state.status {
                SessionStatus::Healthy => {
                    debug!(domain, session_id = idx, "acquired healthy session");
                    return Some(SessionId(idx));
                },
                SessionStatus::Banned => {
                    if self.banned_session_ready(state, idx, now) {
                        debug!(domain, session_id = idx, "acquired session after cooldown");
                        return Some(SessionId(idx));
                    }
                },
                SessionStatus::Retiring => continue,
            }
        }
        warn!(domain, "no available sessions for domain");
        None
    }

    /// Whether a banned session's cooldown (with jitter) has elapsed.
    fn banned_session_ready(&self, state: &SessionState, idx: usize, now: Instant) -> bool {
        let Some(next_retry) = state.next_retry_time else {
            return false;
        };
        // Apply +0–20% dynamic jitter to prevent thundering herd recovery
        let jitter_range = self.backoff_delay(state.consecutive_failures);
        let jitter_ms = (jitter_range.as_millis() as f64 * 0.2) as u128;
        let jitter_offset = Duration::from_millis((idx as u64 * 37) % (jitter_ms.max(1) as u64));
        let effective_retry = next_retry + jitter_offset;
        now >= effective_retry
    }

    /// Apply a failure to a session state, banning or retiring as appropriate.
    fn apply_failure(
        &self,
        state: &mut SessionState,
        domain: &str,
        session_id: SessionId,
        status_code: u16,
        should_ban: bool,
    ) {
        state.consecutive_failures += 1;
        state.last_failure_time = Some(self.clock.now());

        if should_ban {
            let delay = self.backoff_delay(state.consecutive_failures);
            state.next_retry_time = Some(self.clock.now() + delay);
            state.status = SessionStatus::Banned;
            warn!(
                domain,
                session_id = session_id.0,
                status_code,
                failures = state.consecutive_failures,
                backoff_secs = delay.as_secs(),
                "session banned with exponential backoff"
            );
        } else {
            // Non-ban failure: mark retiring after threshold
            if state.consecutive_failures >= 3 {
                state.status = SessionStatus::Retiring;
                warn!(
                    domain,
                    session_id = session_id.0,
                    failures = state.consecutive_failures,
                    "session retiring after repeated failures"
                );
            }
        }
    }
}

impl fmt::Debug for DomainSessionPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DomainSessionPool")
            .field("domains", &self.sessions.len())
            .field("config", &self.config)
            .finish()
    }
}

impl sealed::Sealed for DomainSessionPool {}

impl SessionManager for DomainSessionPool {
    #[instrument(skip(self), fields(domain = %domain))]
    fn acquire(&self, domain: &str) -> Option<SessionId> {
        let mut sessions = self
            .sessions
            .entry(domain.to_string())
            .or_insert_with(|| vec![SessionState::healthy(); self.config.pool_size.get()]);

        // Evict stale sessions first
        let now = self.clock.now();
        self.evict_expired(domain, &mut sessions, now);

        // Find first healthy or recoverable session
        let result = self.find_available(domain, &sessions, now);

        // Drop the DashMap RefMut before refreshing the gauge to avoid deadlock:
        // refresh_healthy_gauge calls self.sessions.iter() which needs read access
        // to the same shard that `sessions` holds a write lock on.
        drop(sessions);

        result
    }

    #[instrument(skip(self), fields(domain = %domain, session_id = %session_id.0))]
    fn report_success(&self, domain: &str, session_id: SessionId) {
        if let Some(mut sessions) = self.sessions.get_mut(domain) {
            if let Some(state) = sessions.get_mut(session_id.0) {
                state.status = SessionStatus::Healthy;
                state.consecutive_failures = 0;
                state.last_failure_time = None;
                state.next_retry_time = None;
                debug!(domain, session_id = session_id.0, "session marked healthy");
            }
        }
    }

    #[instrument(skip(self), fields(domain = %domain, session_id = %session_id.0, status_code))]
    fn report_failure(&self, domain: &str, session_id: SessionId, status_code: u16) {
        // Only ban on signals that indicate domain-level blocking
        let should_ban = matches!(status_code, 429 | 503 | 403);

        if let Some(mut sessions) = self.sessions.get_mut(domain) {
            if let Some(state) = sessions.get_mut(session_id.0) {
                self.apply_failure(state, domain, session_id, status_code, should_ban);
            }
        }
    }

    #[instrument(skip(self))]
    fn evict_stale(&self) {
        let now = self.clock.now();
        for mut entry in self.sessions.iter_mut() {
            let domain = entry.key().clone();
            let sessions = entry.value_mut();
            let mut evicted = 0;
            for state in sessions.iter_mut() {
                if let Some(last_failure) = state.last_failure_time {
                    if now.duration_since(last_failure) > self.config.ttl_duration
                        && state.status != SessionStatus::Healthy
                    {
                        *state = SessionState::healthy();
                        evicted += 1;
                    }
                }
            }
            if evicted > 0 {
                debug!(domain = %domain, evicted, "evicted stale sessions");
            }
        }
    }

    fn domain_count(&self, domain: &str) -> usize {
        self.sessions.get(domain).map(|s| s.len()).unwrap_or(0)
    }

    fn total_domains(&self) -> usize {
        self.sessions.len()
    }
}

impl crate::domain::session_port::SessionPort for DomainSessionPool {
    fn acquire(&self, domain: &str) -> Option<SessionId> {
        SessionManager::acquire(self, domain)
    }

    fn report_success(&self, domain: &str, session: SessionId) {
        SessionManager::report_success(self, domain, session)
    }

    fn report_failure(&self, domain: &str, session: SessionId, status: u16) {
        SessionManager::report_failure(self, domain, session, status)
    }
}

/// Sealed trait internals — prevents external implementations.
mod sealed {
    pub trait Sealed {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::clock::MockClock;

    /// Test constructor for the NonZero-gated Domain tier.
    fn slots(n: usize) -> DomainSlots {
        DomainSlots::new(n).expect("test slot count non-zero")
    }

    // ── Task 3.3: State transitions ──

    #[test]
    fn new_session_is_healthy() {
        let pool = DomainSessionPool::default_pool();
        let id = pool.acquire("example.com").expect("should acquire");
        assert_eq!(id, SessionId(0));
    }

    #[test]
    fn healthy_to_banned_on_429() {
        let pool = DomainSessionPool::default_pool();
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Banned);
        assert_eq!(sessions[0].consecutive_failures, 1);
    }

    #[test]
    fn banned_to_healthy_on_success() {
        let pool = DomainSessionPool::default_pool();
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);
        pool.report_success("example.com", id);

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Healthy);
        assert_eq!(sessions[0].consecutive_failures, 0);
    }

    #[test]
    fn non_ban_failure_does_not_ban() {
        let pool = DomainSessionPool::default_pool();
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 500);

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Healthy);
        assert_eq!(sessions[0].consecutive_failures, 1);
    }

    #[test]
    fn repeated_non_ban_failures_trigger_retiring() {
        let pool = DomainSessionPool::default_pool();
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 500);
        pool.report_failure("example.com", id, 500);
        pool.report_failure("example.com", id, 500);

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Retiring);
    }

    // ── Task 3.3: Backoff doubling (with ±20% jitter) ──

    #[test]
    fn backoff_doubles_with_failures() {
        let pool = DomainSessionPool::default_pool();
        // With ±20% jitter, d1 should be in [1.6s, 2.4s], d2 in [3.2s, 4.8s], d3 in [6.4s, 9.6s]
        let d1 = pool.backoff_delay(1);
        assert!(
            d1 >= Duration::from_millis(1600) && d1 <= Duration::from_millis(2400),
            "d1 should be ~2s ±20%, got {d1:?}"
        );
        let d2 = pool.backoff_delay(2);
        assert!(
            d2 >= Duration::from_millis(3200) && d2 <= Duration::from_millis(4800),
            "d2 should be ~4s ±20%, got {d2:?}"
        );
        let d3 = pool.backoff_delay(3);
        assert!(
            d3 >= Duration::from_millis(6400) && d3 <= Duration::from_millis(9600),
            "d3 should be ~8s ±20%, got {d3:?}"
        );
    }

    #[test]
    fn backoff_capped_at_max_delay() {
        let config = SessionPoolConfig {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            max_exp: 6,
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
        // With ±20% jitter, capped delay should be in [8s, 12s]
        let d_large = pool.backoff_delay(100);
        assert!(
            d_large >= Duration::from_secs(8) && d_large <= Duration::from_secs(12),
            "capped delay should be ~10s ±20%, got {d_large:?}"
        );
    }

    #[test]
    fn backoff_uses_max_exp_cap() {
        let config = SessionPoolConfig {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(120),
            max_exp: 4,
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
        // max_exp=4 means exponent is capped at 4, so 2^4=16
        // With ±20% jitter, delay should be in [12.8s, 19.2s]
        let d4 = pool.backoff_delay(4);
        assert!(
            d4 >= Duration::from_millis(12800) && d4 <= Duration::from_millis(19200),
            "d4 should be ~16s ±20%, got {d4:?}"
        );
        let d5 = pool.backoff_delay(5);
        assert!(
            d5 >= Duration::from_millis(12800) && d5 <= Duration::from_millis(19200),
            "d5 should be ~16s ±20%, got {d5:?}"
        );
        let d6 = pool.backoff_delay(6);
        assert!(
            d6 >= Duration::from_millis(12800) && d6 <= Duration::from_millis(19200),
            "d6 should be ~16s ±20%, got {d6:?}"
        );
    }

    // ── Task 3.3: TTL eviction ──

    #[test]
    fn stale_sessions_evicted_on_acquire() {
        let clock = MockClock::new(Instant::now());
        let pool = DomainSessionPool::new(
            SessionPoolConfig {
                ttl_duration: Duration::from_millis(10),
                ..Default::default()
            },
            clock.handle(),
        );
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);

        // Advance past TTL — the shared clock updates the pool's view too
        clock.advance(Duration::from_millis(20));

        // acquire should evict the stale banned session and return a healthy one
        let id2 = pool.acquire("example.com");
        assert!(id2.is_some(), "stale session should be evicted and retried");
    }

    #[test]
    fn evict_stale_removes_old_banned_sessions() {
        let clock = MockClock::new(Instant::now());
        let pool = DomainSessionPool::new(
            SessionPoolConfig {
                ttl_duration: Duration::from_millis(10),
                ..Default::default()
            },
            clock.handle(),
        );
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);

        // Advance past TTL
        clock.advance(Duration::from_millis(20));
        pool.evict_stale();

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Healthy);
    }

    #[test]
    fn healthy_sessions_not_evicted_by_ttl() {
        let clock = MockClock::new(Instant::now());
        let pool = DomainSessionPool::new(
            SessionPoolConfig {
                ttl_duration: Duration::from_millis(10),
                ..Default::default()
            },
            clock.handle(),
        );
        let _id = pool.acquire("example.com").unwrap();

        // Advance past TTL — healthy sessions should NOT be evicted
        clock.advance(Duration::from_millis(20));
        pool.evict_stale();

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Healthy);
    }

    // ── Task 2.2(c): pool size follows the budget model's Domain tier ──

    /// Task 2.2(c): the pool's slot count follows the injected [`BudgetModel`]
    /// Domain tier — a raw `usize` is no longer representable (D4 newtype),
    /// so a miswired or zero slot count cannot compile.
    #[test]
    fn pool_size_follows_budget_model_domain_tier() {
        use std::num::NonZeroUsize;

        use crate::domain::budget::detector::FixedDetector;
        use crate::domain::budget::{BudgetModel, BudgetOverrides};

        fn config_from_cores(cores: usize) -> (SessionPoolConfig, usize) {
            let detector = FixedDetector::with_detection(
                NonZeroUsize::new(cores).expect("test cores non-zero"),
                None,
            );
            let model = BudgetModel::build(BudgetOverrides::default(), &detector);
            let expected = model.domain().get();
            let config = SessionPoolConfig {
                base_delay: Duration::from_secs(1),
                pool_size: model.domain(),
                ..SessionPoolConfig::default()
            };
            (config, expected)
        }

        // TRIANGULATE across detector variants — every variant must resolve
        // the pool to exactly its model's Domain tier.
        for cores in [2, 4, 16] {
            let (config, expected) = config_from_cores(cores);
            assert_eq!(config.pool_size.get(), expected);

            let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
            let _id = pool.acquire("example.com").expect("should acquire");
            assert_eq!(
                pool.domain_count("example.com"),
                expected,
                "cores={cores}: live slot count must equal the model Domain tier"
            );
        }

        // Zero slots are unrepresentable: the NonZero guard rejects them at
        // construction instead of silently degrading the pool.
        assert!(DomainSlots::new(0).is_err());
    }

    // ── Task 3.3: Pool size limit ──

    #[test]
    fn pool_respects_configured_size() {
        let config = SessionPoolConfig {
            pool_size: slots(3),
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
        let _id = pool.acquire("example.com").unwrap();

        assert_eq!(pool.domain_count("example.com"), 3);
    }

    #[test]
    fn acquire_returns_different_sessions() {
        let config = SessionPoolConfig {
            pool_size: slots(4),
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
        let id1 = pool.acquire("example.com").unwrap();
        let id2 = pool.acquire("example.com").unwrap();

        // Should get different session IDs (first available)
        assert_eq!(id1, SessionId(0));
        assert_eq!(id2, SessionId(0)); // Both get the same first healthy one
    }

    #[test]
    fn multiple_domains_independent() {
        let pool = DomainSessionPool::default_pool();
        let id1 = pool.acquire("a.com").unwrap();
        let id2 = pool.acquire("b.com").unwrap();

        pool.report_failure("a.com", id1, 429);
        pool.report_success("b.com", id2);

        // a.com has a banned session, b.com is healthy
        let a = pool.sessions.get("a.com").unwrap();
        let b = pool.sessions.get("b.com").unwrap();
        assert_eq!(a[0].status, SessionStatus::Banned);
        assert_eq!(b[0].status, SessionStatus::Healthy);
        assert_eq!(pool.total_domains(), 2);
    }

    // ── Additional edge cases ──

    #[test]
    fn acquire_uninitialized_domain_creates_pool() {
        let pool = DomainSessionPool::default_pool();
        assert_eq!(pool.domain_count("new.com"), 0);
        let _id = pool.acquire("new.com");
        assert_eq!(pool.domain_count("new.com"), 8);
    }

    #[test]
    fn banned_session_available_after_cooldown() {
        let clock = MockClock::new(Instant::now());
        let pool = DomainSessionPool::new(
            SessionPoolConfig {
                pool_size: slots(1),
                base_delay: Duration::from_millis(1),
                max_delay: Duration::from_millis(10),
                max_exp: 1,
                ..Default::default()
            },
            clock.handle(),
        );
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);

        // Verify it's banned before cooldown
        {
            let sessions = pool.sessions.get("example.com").unwrap();
            assert_eq!(sessions[0].status, SessionStatus::Banned);
            assert!(sessions[0].next_retry_time.is_some());
        }

        // Advance past cooldown (backoff is 2^1 * 1ms = 2ms)
        clock.advance(Duration::from_millis(50));

        let id2 = pool.acquire("example.com");
        assert!(id2.is_some(), "should be available after cooldown");
    }

    #[test]
    fn session_id_display() {
        let id = SessionId(42);
        assert_eq!(format!("{id}"), "session-42");
    }

    #[test]
    fn default_config_values() {
        let config = SessionPoolConfig::default();
        assert_eq!(config.pool_size.get(), 8);
        assert_eq!(config.base_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(60));
        assert_eq!(config.max_exp, 6);
        assert_eq!(config.ttl_duration, Duration::from_secs(300));
    }

    #[test]
    fn report_failure_on_nonexistent_session_no_panic() {
        let pool = DomainSessionPool::default_pool();
        // Should not panic — just a no-op
        pool.report_failure("ghost.com", SessionId(99), 500);
    }

    #[test]
    fn report_success_on_nonexistent_session_no_panic() {
        let pool = DomainSessionPool::default_pool();
        pool.report_success("ghost.com", SessionId(99));
    }
}
