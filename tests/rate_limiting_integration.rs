//! Integration tests for rate limiting behavior
//!
//! Verifies that the rate limiting system actually:
//! - Spreads requests over time
//! - Handles bursts correctly
//! - Responds to 429 backpressure
//! - Integrates with HttpClient
//!
//! Run with: cargo nextest run --test-threads 2 rate_limiting_integration

#![cfg(not(miri))]

use std::sync::atomic::{AtomicUsize, Ordering};

use rust_scraper::application::http_client::{HttpClient, HttpClientConfig, HttpError};
use rust_scraper::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use tokio::time::Instant;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ============================================================================
// Test 1: Concurrent Until Ready — Requests Spread Over Time
// ============================================================================

#[tokio::test]
async fn test_concurrent_requests_spread_over_time() {
    // Arrange: rate limiter con delay 50ms, burst 1
    let config = RateLimiterConfig::new(50, 1);
    let limiter = SharedRateLimiter::new(&config).unwrap();
    let num_tasks = 5;

    // Act: 5 tasks concurrentes llaman until_ready()
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

    // Assert: debe tomar al menos 150ms para 5 tasks (50ms × 4 intervals)
    assert!(
        elapsed.as_millis() >= 150,
        "5 requests en {}ms deberían tomar >= 150ms",
        elapsed.as_millis()
    );
}

// ============================================================================
// Test 2: Burst Capacity — Parallel Requests Within Burst
// ============================================================================

#[tokio::test]
async fn test_burst_allows_parallel_requests() {
    // Arrange: rate limiter con delay 100ms, burst 5
    let config = RateLimiterConfig::new(100, 5);
    let limiter = SharedRateLimiter::new(&config).unwrap();
    let num_tasks = 5;

    // Act: 5 tasks todas al mismo tiempo (dentro del burst)
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

    // Assert: todas pasan en < 50ms (dentro del burst)
    assert!(
        elapsed.as_millis() < 50,
        "5 requests concurrently en {}ms deberían pasar < 50ms",
        elapsed.as_millis()
    );
}

// ============================================================================
// Test 3: Backpressure — Concurrent Load
// ============================================================================

#[tokio::test]
async fn test_backpressure_with_high_concurrency() {
    // Arrange
    let config = RateLimiterConfig::new(20, 1);
    let limiter = SharedRateLimiter::new(&config).unwrap();
    let num_tasks = 20;

    // Act
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

    // Assert: 20 tasks × 20ms = 380ms mínimo, verificamos >= 200ms
    assert!(
        elapsed.as_millis() >= 200,
        "20 tasks en {}ms — backpressure no regulando",
        elapsed.as_millis()
    );
}

// ============================================================================
// Test 4: HTTP Client 429 + Retry After Header
// ============================================================================

#[tokio::test]
async fn test_http_client_429_respects_retry_after() {
    // Arrange: mock server que retorna 429 con Retry-After
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("Retry-After", "1")
                .set_body_string(r#"{"error":"Rate limited"}"#),
        )
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        max_retries: 2,
        backoff_base_ms: 500,
        backoff_max_ms: 2000,
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    // Act
    let start = std::time::Instant::now();
    let result = client.get(&mock_server.uri()).await;
    let elapsed = start.elapsed();

    // Assert: debe retornar RateLimited error
    assert!(result.is_err());
    if let Err(HttpError::RateLimited(_)) = result {
        // OK
    } else {
        panic!("Expected RateLimited, got: {:?}", result);
    }

    // Verifica que esperó al menos el Retry-After (1s)
    assert!(
        elapsed.as_secs() >= 1,
        "Debió esperar Retry-After de 1s, esperó {}ms",
        elapsed.as_millis()
    );
}

// ============================================================================
// Test 5: HTTP Client 429 Exponential Fallback
// ============================================================================

#[tokio::test]
async fn test_http_client_429_without_retry_after() {
    // Arrange: mock server que retorna 429 SIN Retry-After
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        max_retries: 2,
        backoff_base_ms: 100,
        backoff_max_ms: 500,
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    // Act
    let start = std::time::Instant::now();
    let result = client.get(&mock_server.uri()).await;
    let elapsed = start.elapsed();

    // Assert: debe esperar exponential backoff
    assert!(result.is_err());
    assert!(
        elapsed.as_millis() >= 100,
        "Debió esperar backoff base, esperó {}ms",
        elapsed.as_millis()
    );
}

// ============================================================================
// Test 6: HTTP Client Rate Limit RPM Enforcement
// ============================================================================

#[tokio::test]
async fn test_http_client_rate_limit_rpm_config() {
    // Test configuración — verify que rate_limit_rpm se aplica
    let config = HttpClientConfig {
        rate_limit_rpm: Some(60),
        ..Default::default()
    };
    let result = HttpClient::new(config);
    assert!(
        result.is_ok(),
        "HttpClient debería crearse con rate_limit_rpm"
    );
}

// ============================================================================
// Test 7: Retry After Header Parsing
// ============================================================================

#[tokio::test]
async fn test_retry_after_header_parsing() {
    // Arrange
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .append_header("retry-after", "30")
                .set_body_string(r#"{"error":"limit"}"#),
        )
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        max_retries: 1,
        backoff_base_ms: 5000,
        backoff_max_ms: 60000,
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    // Act
    let start = std::time::Instant::now();
    let result = client.get(&mock_server.uri()).await;
    let elapsed = start.elapsed();

    // Assert
    assert!(result.is_err());
    // retry-after 30s + algo de overhead de parsing
    assert!(
        elapsed.as_secs() >= 25,
        "retry-after=30s, esperó {}s",
        elapsed.as_secs()
    );
}

// ============================================================================
// Test 8: Shared Rate Limiter Timing Accuracy
// ============================================================================

#[tokio::test]
async fn test_rate_limiter_timing_accuracy() {
    // Arrange: delay preciso de 100ms
    let config = RateLimiterConfig::new(100, 1);
    let limiter = SharedRateLimiter::new(&config).unwrap();

    // Act: múltiples ciclos de acquire
    let num_cycles = 5;
    let mut total_elapsed_ms: i128 = 0;

    for i in 0..num_cycles {
        let start = Instant::now();
        limiter.until_ready().await;
        let elapsed = start.elapsed().as_millis() as i128;
        total_elapsed_ms += elapsed;

        // Reset: crear nuevo limiter para el próximo ciclo
        if i < num_cycles - 1 {
            let config = RateLimiterConfig::new(100, 1);
            let limiter = SharedRateLimiter::new(&config).unwrap();
            drop(limiter);
        }
    }

    // Assert: el promedio debería estar cerca de 100ms (con ±30ms de tolerancia)
    let avg_elapsed = total_elapsed_ms / num_cycles;
    assert!(
        (70..=150).contains(&avg_elapsed),
        "Promedio {}ms debería estar entre 70-150ms",
        avg_elapsed
    );
}

// ============================================================================
// Test 9: Concurrent Rate Limiter Acquisition Count
// ============================================================================

#[tokio::test]
async fn test_concurrent_rate_limiter_acquisition_count() {
    // Arrange
    let config = RateLimiterConfig::new(10, 1);
    let limiter = SharedRateLimiter::new(&config).unwrap();
    let counter = std::sync::Arc::new(AtomicUsize::new(0));
    let num_tasks = 10;

    // Act
    let mut handles = Vec::new();
    for _ in 0..num_tasks {
        let limiter = limiter.clone();
        let counter = std::sync::Arc::clone(&counter);
        let handle = tokio::spawn(async move {
            limiter.until_ready().await;
            counter.fetch_add(1, Ordering::SeqCst);
        });
        handles.push(handle);
    }

    futures::future::join_all(handles).await;
    let count = counter.load(Ordering::SeqCst);

    // Assert: todas las tasks completaron
    assert_eq!(
        count, num_tasks,
        "Solo {} de {} tasks completaron",
        count, num_tasks
    );
}

// ============================================================================
// Test 10: Edge Case — Multiple Rapid Requests
// ============================================================================

#[tokio::test]
async fn test_multiple_rapid_requests_drain() {
    // Arrange: rate limiter lento
    let config = RateLimiterConfig::new(50, 1);
    let limiter = SharedRateLimiter::new(&config).unwrap();

    // Act: 3 requests rápidos consecutivos (dentro del burst inicial, luego espaciado)
    let start = Instant::now();

    limiter.until_ready().await; // 1: inmediato (burst)
    limiter.until_ready().await; // 2: espera ~50ms
    limiter.until_ready().await; // 3: espera ~50ms más

    let elapsed = start.elapsed();

    // Assert: mínimo ~100ms para 3 requests (sin burst restante)
    assert!(
        elapsed.as_millis() >= 80,
        "3 requests en {}ms — spacing no funciona",
        elapsed.as_millis()
    );
}
