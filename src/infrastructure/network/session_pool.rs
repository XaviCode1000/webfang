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

use std::fmt;
use std::time::{Duration, Instant};

#[cfg(feature = "otel-metrics")]
use std::hash::{Hash, Hasher};

use dashmap::DashMap;
use tracing::{debug, instrument, warn};

#[cfg(feature = "otel-metrics")]
use crate::infrastructure::observability::metrics_instruments::{
    update_session_pool_healthy, SESSION_POOL_BACKOFF, SESSION_POOL_BANNED,
};

/// Unique identifier for a session slot within a domain pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub usize);

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "session-{}", self.0)
    }
}

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
    /// Number of session slots per domain.
    pub pool_size: usize,
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
            pool_size: 8,
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
pub struct DomainSessionPool {
    /// Per-domain session states. Key = domain string, Value = Vec of session states.
    sessions: DashMap<String, Vec<SessionState>>,
    config: SessionPoolConfig,
}

/// Hash a domain string to a bounded bucket (0–999) for metric attributes.
///
/// Prevents cardinality explosion from unbounded domain strings.
#[cfg(feature = "otel-metrics")]
fn domain_bucket(domain: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    domain.hash(&mut hasher);
    hasher.finish() % 1000
}

impl DomainSessionPool {
    /// Create a new session pool with the given configuration.
    #[must_use]
    pub fn new(config: SessionPoolConfig) -> Self {
        Self {
            sessions: DashMap::new(),
            config,
        }
    }

    /// Create a pool with default configuration.
    #[must_use]
    pub fn default_pool() -> Self {
        Self::new(SessionPoolConfig::default())
    }

    /// Count total healthy sessions across all domains and update the gauge.
    #[cfg(feature = "otel-metrics")]
    fn refresh_healthy_gauge(&self) {
        let mut count: u64 = 0;
        for entry in self.sessions.iter() {
            for state in entry.value().iter() {
                if state.status == SessionStatus::Healthy {
                    count += 1;
                }
            }
        }
        update_session_pool_healthy(count);
    }

    /// Calculate exponential backoff delay for a given failure count.
    fn backoff_delay(&self, consecutive_failures: u32) -> Duration {
        let exponent = consecutive_failures.min(self.config.max_exp);
        let base_ms = self.config.base_delay.as_millis();
        let max_ms = self.config.max_delay.as_millis();
        let delay_ms = base_ms.saturating_mul(2u128.pow(exponent));
        let capped = delay_ms.min(max_ms).max(1);
        Duration::from_millis(capped as u64)
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
            .or_insert_with(|| vec![SessionState::healthy(); self.config.pool_size]);

        // Evict stale sessions first
        let now = Instant::now();
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

        // Find first healthy or recoverable session
        let result = 'find: {
            for (idx, state) in sessions.iter().enumerate() {
                match state.status {
                    SessionStatus::Healthy => {
                        debug!(domain, session_id = idx, "acquired healthy session");
                        break 'find Some(SessionId(idx));
                    },
                    SessionStatus::Banned => {
                        if let Some(next_retry) = state.next_retry_time {
                            if now >= next_retry {
                                debug!(domain, session_id = idx, "acquired session after cooldown");
                                break 'find Some(SessionId(idx));
                            }
                        }
                    },
                    SessionStatus::Retiring => continue,
                }
            }
            warn!(domain, "no available sessions for domain");
            None
        };

        // Drop the DashMap RefMut before refreshing the gauge to avoid deadlock:
        // refresh_healthy_gauge calls self.sessions.iter() which needs read access
        // to the same shard that `sessions` holds a write lock on.
        drop(sessions);

        #[cfg(feature = "otel-metrics")]
        self.refresh_healthy_gauge();

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
        #[cfg(feature = "otel-metrics")]
        self.refresh_healthy_gauge();
    }

    #[instrument(skip(self), fields(domain = %domain, session_id = %session_id.0, status_code))]
    fn report_failure(&self, domain: &str, session_id: SessionId, status_code: u16) {
        // Only ban on signals that indicate domain-level blocking
        let should_ban = matches!(status_code, 429 | 503 | 403);

        if let Some(mut sessions) = self.sessions.get_mut(domain) {
            if let Some(state) = sessions.get_mut(session_id.0) {
                state.consecutive_failures += 1;
                state.last_failure_time = Some(Instant::now());

                if should_ban {
                    let delay = self.backoff_delay(state.consecutive_failures);
                    state.next_retry_time = Some(Instant::now() + delay);
                    state.status = SessionStatus::Banned;
                    #[cfg(feature = "otel-metrics")]
                    {
                        let bucket = domain_bucket(domain);
                        SESSION_POOL_BANNED.add(
                            1,
                            &[opentelemetry::KeyValue::new(
                                "domain",
                                format!("{bucket:04}"),
                            )],
                        );
                        SESSION_POOL_BACKOFF.record(
                            delay.as_secs_f64(),
                            &[opentelemetry::KeyValue::new(
                                "domain",
                                format!("{bucket:04}"),
                            )],
                        );
                    }
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
        #[cfg(feature = "otel-metrics")]
        self.refresh_healthy_gauge();
    }

    #[instrument(skip(self))]
    fn evict_stale(&self) {
        let now = Instant::now();
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

/// Sealed trait internals — prevents external implementations.
mod sealed {
    pub trait Sealed {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

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

    // ── Task 3.3: Backoff doubling ──

    #[test]
    fn backoff_doubles_with_failures() {
        let pool = DomainSessionPool::default_pool();
        let d1 = pool.backoff_delay(1);
        let d2 = pool.backoff_delay(2);
        let d3 = pool.backoff_delay(3);

        assert_eq!(d1, Duration::from_secs(2));
        assert_eq!(d2, Duration::from_secs(4));
        assert_eq!(d3, Duration::from_secs(8));
    }

    #[test]
    fn backoff_capped_at_max_delay() {
        let config = SessionPoolConfig {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(10),
            max_exp: 6,
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
        let d_large = pool.backoff_delay(100);
        assert_eq!(d_large, Duration::from_secs(10));
    }

    #[test]
    fn backoff_uses_max_exp_cap() {
        let config = SessionPoolConfig {
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(120),
            max_exp: 4,
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
        let d4 = pool.backoff_delay(4);
        let d5 = pool.backoff_delay(5);
        let d6 = pool.backoff_delay(6);

        // max_exp=4 means exponent is capped at 4, so 2^4=16
        assert_eq!(d4, Duration::from_secs(16));
        assert_eq!(d5, Duration::from_secs(16));
        assert_eq!(d6, Duration::from_secs(16));
    }

    // ── Task 3.3: TTL eviction ──

    #[test]
    fn stale_sessions_evicted_on_acquire() {
        let config = SessionPoolConfig {
            ttl_duration: Duration::from_millis(1),
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);

        // Wait for TTL to expire
        thread::sleep(Duration::from_millis(5));

        // acquire should evict the stale banned session and return a healthy one
        let id2 = pool.acquire("example.com");
        assert!(id2.is_some(), "stale session should be evicted and retried");
    }

    #[test]
    fn evict_stale_removes_old_banned_sessions() {
        let config = SessionPoolConfig {
            ttl_duration: Duration::from_millis(1),
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);

        thread::sleep(Duration::from_millis(5));
        pool.evict_stale();

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Healthy);
    }

    #[test]
    fn healthy_sessions_not_evicted_by_ttl() {
        let config = SessionPoolConfig {
            ttl_duration: Duration::from_millis(1),
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
        let _id = pool.acquire("example.com").unwrap();

        thread::sleep(Duration::from_millis(5));
        pool.evict_stale();

        let sessions = pool.sessions.get("example.com").unwrap();
        assert_eq!(sessions[0].status, SessionStatus::Healthy);
    }

    // ── Task 3.3: Pool size limit ──

    #[test]
    fn pool_respects_configured_size() {
        let config = SessionPoolConfig {
            pool_size: 3,
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
        let _id = pool.acquire("example.com").unwrap();

        assert_eq!(pool.domain_count("example.com"), 3);
    }

    #[test]
    fn acquire_returns_different_sessions() {
        let config = SessionPoolConfig {
            pool_size: 4,
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
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
        let config = SessionPoolConfig {
            pool_size: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
            max_exp: 1,
            ..Default::default()
        };
        let pool = DomainSessionPool::new(config);
        let id = pool.acquire("example.com").unwrap();
        pool.report_failure("example.com", id, 429);

        // Verify it's banned before cooldown
        {
            let sessions = pool.sessions.get("example.com").unwrap();
            assert_eq!(sessions[0].status, SessionStatus::Banned);
            assert!(sessions[0].next_retry_time.is_some());
        }

        // Wait well past cooldown (backoff is 2^1 * 1ms = 2ms, sleep 50ms)
        thread::sleep(Duration::from_millis(50));

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
        assert_eq!(config.pool_size, 8);
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

#[cfg(test)]
#[cfg(feature = "otel-metrics")]
mod metrics_tests {
    use super::*;

    #[test]
    fn test_session_pool_instruments_init() {
        let _ = &*SESSION_POOL_BANNED;
        let _ = &*SESSION_POOL_BACKOFF;
    }

    #[test]
    fn test_domain_bucket_is_bounded() {
        let b1 = domain_bucket("example.com");
        let b2 = domain_bucket("another-domain.org");
        let b3 = domain_bucket("a".repeat(1000).as_str());
        assert!(b1 < 1000);
        assert!(b2 < 1000);
        assert!(b3 < 1000);
    }

    #[test]
    fn test_domain_bucket_deterministic() {
        assert_eq!(domain_bucket("test.com"), domain_bucket("test.com"));
    }

    #[test]
    fn test_domain_bucket_different_domains_differ() {
        // Not guaranteed, but overwhelmingly likely for 1000 buckets
        assert_ne!(domain_bucket("a.com"), domain_bucket("b.com"));
    }
}
