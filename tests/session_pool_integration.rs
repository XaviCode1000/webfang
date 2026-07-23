//! Integration tests for DomainSessionPool — concurrent access patterns.
//!
//! Exercises acquire/report_success/report_failure lifecycle across multiple
//! threads, verifying banning, health tracking, and cooldown behavior.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use webfang_core::domain::clock::SystemClock;
use webfang_core::{DomainSessionPool, SessionId, SessionManager, SessionPoolConfig};

// ===== ACQUIRE / RELEASE CYCLE =====

/// Acquire → work → report_success → acquire again succeeds.
#[test]
fn test_acquire_success_acquire_cycle() {
    let config = SessionPoolConfig {
        pool_size: 1,
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));

    let id = pool.acquire("example.com").expect("should acquire");
    pool.report_success("example.com", id);

    // Should be able to acquire again (session is healthy)
    let id2 = pool.acquire("example.com").expect("should acquire again");
    assert_eq!(id2, SessionId(0));
}

// ===== BANNING =====

/// Banning on 429 makes session unavailable.
#[test]
fn test_ban_on_429() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));

    let id = pool.acquire("example.com").expect("should acquire");
    pool.report_failure("example.com", id, 429);

    // With pool_size=1 and long cooldown, acquire should return None
    assert!(
        pool.acquire("example.com").is_none(),
        "banned session should not be available"
    );
}

/// Banning on 403 makes session unavailable.
#[test]
fn test_ban_on_403() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));

    let id = pool.acquire("example.com").expect("should acquire");
    pool.report_failure("example.com", id, 403);

    assert!(pool.acquire("example.com").is_none());
}

/// Banning on 503 makes session unavailable.
#[test]
fn test_ban_on_503() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));

    let id = pool.acquire("example.com").expect("should acquire");
    pool.report_failure("example.com", id, 503);

    assert!(pool.acquire("example.com").is_none());
}

/// Non-ban failure (500) does not ban session.
#[test]
fn test_non_ban_failure_no_ban() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").expect("should acquire");
    pool.report_failure("example.com", id, 500);

    // Session should still be available (not banned)
    assert!(
        pool.acquire("example.com").is_some(),
        "500 should not ban session"
    );
}

/// Success resets failure count after ban.
#[test]
fn test_success_resets_after_ban() {
    let pool = DomainSessionPool::default_pool();
    let id = pool.acquire("example.com").expect("should acquire");
    pool.report_failure("example.com", id, 500);
    pool.report_failure("example.com", id, 500);
    pool.report_success("example.com", id);

    // After success, should still be available
    let id2 = pool.acquire("example.com").expect("should acquire");
    pool.report_failure("example.com", id2, 500);
    pool.report_failure("example.com", id2, 500);
    // Still not banned (only 2 failures after success reset, need 3 for retiring)
    assert!(
        pool.acquire("example.com").is_some(),
        "should still be available"
    );
}

// ===== COOLDOWN RECOVERY =====

/// Banned session recovers after cooldown.
#[test]
fn test_banned_session_recovers_after_cooldown() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_millis(1),
        max_delay: Duration::from_millis(10),
        max_exp: 1,
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));

    let id = pool.acquire("example.com").expect("should acquire");
    pool.report_failure("example.com", id, 429);

    // Immediately: banned
    assert!(pool.acquire("example.com").is_none());

    // Wait past cooldown (2^1 * 1ms = 2ms)
    thread::sleep(Duration::from_millis(50));

    assert!(
        pool.acquire("example.com").is_some(),
        "should be available after cooldown"
    );
}

// ===== CONCURRENT ACCESS =====

/// Multiple threads accessing the pool simultaneously don't deadlock.
#[test]
fn test_concurrent_access_no_deadlock() {
    let pool = Arc::new(DomainSessionPool::default_pool());
    let mut handles = Vec::new();

    for i in 0..20 {
        let pool: Arc<DomainSessionPool> = Arc::clone(&pool);
        handles.push(thread::spawn(move || {
            let domain = format!("task{i}.example.com");
            let result = pool.acquire(&domain);
            assert!(result.is_some(), "concurrent acquire should succeed");
            if let Some(id) = result {
                pool.report_success(&domain, id);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("task should complete without panic");
    }

    assert_eq!(pool.total_domains(), 20, "should track all 20 domains");
}

/// Concurrent failures on same domain.
#[test]
fn test_concurrent_failures_same_domain() {
    let pool = Arc::new(DomainSessionPool::default_pool());
    let mut handles = Vec::new();

    for _ in 0..10 {
        let pool: Arc<DomainSessionPool> = Arc::clone(&pool);
        handles.push(thread::spawn(move || {
            if let Some(id) = pool.acquire("shared.example.com") {
                pool.report_failure("shared.example.com", id, 500);
            }
        }));
    }

    for handle in handles {
        handle.join().expect("task should complete");
    }

    // With pool_size=8, some sessions may still be available
    // Just verify no panic occurred and domains are tracked
    assert!(pool.total_domains() >= 1);
}

// ===== MULTIPLE DOMAINS =====

/// Domains are tracked independently — one domain's failures don't affect another.
#[test]
fn test_domains_independent_failure_tracking() {
    let config = SessionPoolConfig {
        pool_size: 1,
        base_delay: Duration::from_secs(60),
        ..Default::default()
    };
    let pool = DomainSessionPool::new(config, Arc::new(SystemClock));

    // Ban domain A
    let id_a = pool.acquire("a.example.com").expect("should acquire");
    pool.report_failure("a.example.com", id_a, 429);

    // Domain A is banned, domain B should be unaffected
    assert!(pool.acquire("a.example.com").is_none());
    assert!(pool.acquire("b.example.com").is_some());

    assert_eq!(pool.total_domains(), 2);
}

// ===== EDGE CASES =====

/// Acquire on uninitialized domain creates pool.
#[test]
fn test_acquire_creates_domain_pool() {
    let pool = DomainSessionPool::default_pool();
    assert_eq!(pool.domain_count("new.com"), 0);
    let _id = pool.acquire("new.com");
    assert_eq!(pool.domain_count("new.com"), 8);
}

/// report_failure on nonexistent session doesn't panic.
#[test]
fn test_report_failure_nonexistent_no_panic() {
    let pool = DomainSessionPool::default_pool();
    pool.report_failure("ghost.com", SessionId(99), 500);
}

/// report_success on nonexistent session doesn't panic.
#[test]
fn test_report_success_nonexistent_no_panic() {
    let pool = DomainSessionPool::default_pool();
    pool.report_success("ghost.com", SessionId(99));
}

/// SessionId display format.
#[test]
fn test_session_id_display() {
    assert_eq!(format!("{}", SessionId(42)), "session-42");
}

/// Default config values.
#[test]
fn test_default_config_values() {
    let config = SessionPoolConfig::default();
    assert_eq!(config.pool_size, 8);
    assert_eq!(config.base_delay, Duration::from_secs(1));
    assert_eq!(config.max_delay, Duration::from_secs(60));
    assert_eq!(config.max_exp, 6);
    assert_eq!(config.ttl_duration, Duration::from_secs(300));
}
