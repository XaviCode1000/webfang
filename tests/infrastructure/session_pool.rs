//! Integration tests for DomainSessionPool — public API only.
//!
//! Uses SessionManager trait: acquire, report_success, report_failure,
//! evict_stale, domain_count, total_domains.

use std::sync::Arc;
use std::thread;
use std::time::Duration;
use webfang_core::domain::clock::SystemClock;
use webfang_core::infrastructure::network::session_pool::{
    DomainSessionPool, SessionId, SessionManager, SessionPoolConfig,
};

// ── Acquire / release cycle ───────────────────────────────────────────────

#[test]
fn acquire_returns_session_id() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").expect("should acquire");
    assert_eq!(id, SessionId(0));
}

#[test]
fn acquire_success_acquire_cycle() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").unwrap();
    pool.report_success("example.com", id);

    // Should be able to acquire again (session is healthy)
    let id2 = pool.acquire("example.com").unwrap();
    assert_eq!(id2, SessionId(0));
}

// ── Failure banning ───────────────────────────────────────────────────────

#[test]
fn ban_on_429_makes_session_unavailable() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    // With pool_size=1 and long cooldown, acquire should fail
    let id2 = pool.acquire("example.com");
    assert!(id2.is_none(), "banned session should not be available");
}

#[test]
fn ban_on_403_makes_session_unavailable() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 403);

    let id2 = pool.acquire("example.com");
    assert!(id2.is_none());
}

#[test]
fn ban_on_503_makes_session_unavailable() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 503);

    let id2 = pool.acquire("example.com");
    assert!(id2.is_none());
}

#[test]
fn non_ban_failure_500_does_not_ban() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 500);

    // Session should still be available (not banned)
    let id2 = pool.acquire("example.com");
    assert!(id2.is_some(), "500 should not ban session");
}

#[test]
fn success_resets_failure_count() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 500);
    pool.report_failure("example.com", id, 500);
    pool.report_success("example.com", id);

    // After success, failures should be reset
    // Subsequent 500s would need 3 more to trigger retiring
    let id2 = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id2, 500);
    pool.report_failure("example.com", id2, 500);
    // Still not retired (only 2 failures after success reset)
    let id3 = pool.acquire("example.com");
    assert!(id3.is_some(), "should still be available after 2 failures");
}

// ── Cooldown recovery ─────────────────────────────────────────────────────

#[test]
#[ignore = "timing-sensitive: run with cargo test -- --ignored"]
fn banned_session_recovers_after_cooldown() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(10),
        max_exp: 1,
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    // Immediately: banned
    assert!(pool.acquire("example.com").is_none());

    // NOTE: This sleep is intentionally kept — the pool uses SystemClock and
    // there's no FakeClock injection point in SessionPoolConfig. Eliminating
    // this requires injecting a clock abstraction (trait object or feature-gated
    // clock), which is a design change beyond this refactor scope.
    // Wait past cooldown (2^1 * 1ms = 2ms)
    thread::sleep(Duration::from_millis(50));

    let id2 = pool.acquire("example.com");
    assert!(id2.is_some(), "should be available after cooldown");
}

// ── TTL eviction ──────────────────────────────────────────────────────────

#[test]
#[ignore = "timing-sensitive: run with cargo test -- --ignored"]
fn stale_sessions_evicted_on_acquire() {
    let config = SessionPoolConfig {
        pool_size: 1,
        ttl_duration: Duration::from_millis(1),
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    // NOTE: Same as above — SystemClock only, no FakeClock available yet.
    // This sleep simulates time passing for TTL expiry.
    thread::sleep(Duration::from_millis(5));

    // TTL expired — acquire should evict stale and return healthy session
    let id2 = pool.acquire("example.com");
    assert!(id2.is_some(), "stale session should be evicted via TTL");
}

#[test]
#[ignore = "timing-sensitive: run with cargo test -- --ignored"]
fn evict_stale_recovers_banned_sessions() {
    let config = SessionPoolConfig {
        pool_size: 1,
        ttl_duration: Duration::from_millis(1),
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let id = pool.acquire("example.com").unwrap();
    pool.report_failure("example.com", id, 429);

    // NOTE: Same as above — SystemClock only, no FakeClock available yet.
    // This sleep simulates time passing for TTL expiry.
    thread::sleep(Duration::from_millis(5));
    pool.evict_stale();

    // After explicit eviction, session should be available
    let id2 = pool.acquire("example.com");
    assert!(id2.is_some(), "evict_stale should recover banned session");
}

// ── Pool size and domain tracking ─────────────────────────────────────────

#[test]
fn pool_respects_configured_size() {
    let config = SessionPoolConfig {
        pool_size: 3,
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));
    let _id = pool.acquire("example.com").unwrap();
    assert_eq!(pool.domain_count("example.com"), 3);
}

#[test]
fn acquire_creates_domain_pool() {
    let pool = DomainSessionPool::default_pool();
    assert_eq!(pool.domain_count("new.com"), 0);
    let _id = pool.acquire("new.com");
    assert_eq!(pool.domain_count("new.com"), 8);
}

// ── Multi-domain independence ─────────────────────────────────────────────

#[test]
fn domains_independent_failure_tracking() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));

    let id_a = pool.acquire("a.com").unwrap();
    pool.report_failure("a.com", id_a, 429);

    // a.com is banned, b.com should be unaffected
    let id_b = pool.acquire("b.com");
    assert!(id_b.is_some(), "b.com should be healthy");
    assert_eq!(pool.total_domains(), 2);
}

#[test]
fn concurrent_domains_tracked_separately() {
    let pool = DomainSessionPool::default_pool();
    let domains = ["x.com", "y.com", "z.com"];

    for d in &domains {
        let id = pool.acquire(d).unwrap();
        pool.report_success(d, id);
    }

    assert_eq!(pool.total_domains(), 3);
    for d in &domains {
        assert_eq!(pool.domain_count(d), 8);
    }
}

// ── Edge cases ────────────────────────────────────────────────────────────

#[test]
fn report_failure_on_nonexistent_session_no_panic() {
    let pool = DomainSessionPool::default_pool();
    pool.report_failure("ghost.com", SessionId(99), 500);
}

#[test]
fn report_success_on_nonexistent_session_no_panic() {
    let pool = DomainSessionPool::default_pool();
    pool.report_success("ghost.com", SessionId(99));
}

#[test]
fn session_id_display() {
    assert_eq!(format!("{}", SessionId(42)), "session-42");
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
