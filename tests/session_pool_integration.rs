//! Integration tests for DomainSessionPool — concurrent access patterns.
//!
//! Exercises acquire/report_success/report_failure lifecycle across multiple
//! async tasks, verifying rate limiting, health tracking, and cooldown behavior.

use webfang::DomainSessionPool;
use std::sync::Arc;
use std::time::Duration;

// ===== ACQUIRE / RELEASE CYCLE =====

/// Acquire → work → report_success → acquire again succeeds.
#[tokio::test]
async fn test_acquire_success_acquire_cycle() {
    let pool = DomainSessionPool::new(Duration::from_millis(50), 5);

    // First acquire always succeeds
    assert!(pool.acquire("example.com").await.unwrap());
    pool.report_success("example.com").await;

    // Wait for cooldown
    tokio::time::sleep(Duration::from_millis(60)).await;

    // Second acquire after cooldown succeeds
    assert!(pool.acquire("example.com").await.unwrap());
    pool.report_success("example.com").await;

    let (requests, failures) = pool.stats("example.com").await.unwrap();
    assert_eq!(requests, 2, "should have 2 total requests");
    assert_eq!(failures, 0, "should have 0 failures");
}

// ===== HEALTH CHECK =====

/// Domain becomes unhealthy after max_failures consecutive failures.
#[tokio::test]
async fn test_domain_unhealthy_after_max_failures() {
    let pool = DomainSessionPool::new(Duration::from_millis(0), 3);

    // Report 3 failures (max_failures = 3)
    pool.report_failure("bad.example.com").await;
    pool.report_failure("bad.example.com").await;
    pool.report_failure("bad.example.com").await;

    assert!(
        !pool.is_healthy("bad.example.com").await,
        "domain should be unhealthy after max failures"
    );

    // acquire should return Err for unhealthy domain
    let result = pool.acquire("bad.example.com").await;
    assert!(result.is_err(), "acquire should fail for unhealthy domain");
}

/// Domain recovers after a successful request resets failure count.
#[tokio::test]
async fn test_domain_recovers_after_success() {
    let pool = DomainSessionPool::new(Duration::from_millis(0), 3);

    // 2 failures (below threshold)
    pool.report_failure("recover.example.com").await;
    pool.report_failure("recover.example.com").await;
    assert!(pool.is_healthy("recover.example.com").await);

    // Success resets failures
    pool.report_success("recover.example.com").await;
    assert!(pool.is_healthy("recover.example.com").await);

    let (_, failures) = pool.stats("recover.example.com").await.unwrap();
    assert_eq!(failures, 0, "failures should be reset to 0");
}

/// Unknown domain is considered healthy by default.
#[tokio::test]
async fn test_unknown_domain_healthy() {
    let pool = DomainSessionPool::new(Duration::from_millis(0), 5);
    assert!(
        pool.is_healthy("unknown.example.com").await,
        "unknown domain should be healthy"
    );
}

// ===== COOLDOWN ENFORCEMENT =====

/// Rapid successive requests to same domain get rate-limited.
#[tokio::test]
async fn test_cooldown_defers_rapid_requests() {
    let pool = DomainSessionPool::new(Duration::from_secs(60), 5);

    assert!(pool.acquire("fast.example.com").await.unwrap());
    // Immediate second request should be deferred
    assert!(
        !pool.acquire("fast.example.com").await.unwrap(),
        "second request within cooldown should be deferred"
    );
}

/// After cooldown expires, requests succeed again.
#[tokio::test]
async fn test_cooldown_expires() {
    let pool = DomainSessionPool::new(Duration::from_millis(10), 5);

    assert!(pool.acquire("expire.example.com").await.unwrap());
    tokio::time::sleep(Duration::from_millis(15)).await;
    assert!(
        pool.acquire("expire.example.com").await.unwrap(),
        "request after cooldown should succeed"
    );
}

// ===== CONCURRENT ACCESS =====

/// Multiple async tasks accessing the pool simultaneously don't deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_access_no_deadlock() {
    let pool = Arc::new(DomainSessionPool::new(Duration::from_millis(5), 10));
    let mut handles = Vec::new();

    for i in 0..20 {
        let pool = Arc::clone(&pool);
        handles.push(tokio::spawn(async move {
            let domain = format!("task{i}.example.com");
            let result = pool.acquire(&domain).await;
            assert!(result.is_ok(), "concurrent acquire should not error");
            if result.unwrap() {
                pool.report_success(&domain).await;
            }
        }));
    }

    for handle in handles {
        handle.await.expect("task should complete without panic");
    }

    assert_eq!(pool.domain_count().await, 20, "should track all 20 domains");
}

/// Concurrent failures on same domain don't race past the threshold.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_failures_same_domain() {
    let pool = Arc::new(DomainSessionPool::new(Duration::from_millis(0), 5));
    let mut handles = Vec::new();

    for _ in 0..10 {
        let pool = Arc::clone(&pool);
        handles.push(tokio::spawn(async move {
            // Each task reports a failure — some may succeed, some may get Err
            let _ = pool.acquire("shared.example.com").await;
            pool.report_failure("shared.example.com").await;
        }));
    }

    for handle in handles {
        handle.await.expect("task should complete");
    }

    // After 10 failures (all > max_failures=5), domain should be unhealthy
    assert!(
        !pool.is_healthy("shared.example.com").await,
        "domain with many concurrent failures should be unhealthy"
    );
}

// ===== MULTIPLE DOMAINS =====

/// Domains are tracked independently — one domain's failures don't affect another.
#[tokio::test]
async fn test_domains_independent_failure_tracking() {
    let pool = DomainSessionPool::new(Duration::from_millis(0), 2);

    // Make domain A unhealthy
    pool.report_failure("a.example.com").await;
    pool.report_failure("a.example.com").await;
    assert!(!pool.is_healthy("a.example.com").await);

    // Domain B should still be healthy
    assert!(pool.is_healthy("b.example.com").await);
    assert!(pool.acquire("b.example.com").await.unwrap());

    assert_eq!(pool.domain_count().await, 2);
}
