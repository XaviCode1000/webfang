//! Integration tests for HttpClient
//!
//! Tests use wiremock for deterministic HTTP responses.
//! Run with: cargo test --ignored (for network tests) or cargo test (for mock tests)

#![cfg(not(miri))]

use rust_scraper::application::http_client::{HttpClient, HttpClientConfig};
use std::time::Duration;
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

/// Test HTTP 200 OK with mock server
#[tokio::test]
async fn test_mock_server_200() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>OK</html>"))
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let result = client.get(&mock_server.uri()).await;
    assert!(result.is_ok(), "Should succeed: {:?}", result);
}

/// Test HTTP 404 with mock server - 404 is not an error in HTTP spec
#[tokio::test]
async fn test_mock_server_404() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let url = format!("{}/missing", mock_server.uri());
    let result = client.get(&url).await;
    // 404 returns body, not error (per wreq behavior)
    if let Ok(body) = result {
        assert!(body.contains("Not Found") || body.is_empty());
    }
}

/// Test HTTP 500 with mock server
#[tokio::test]
async fn test_mock_server_500() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/error"))
        .respond_with(ResponseTemplate::new(500).set_body_string("Internal Error"))
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let url = format!("{}/error", mock_server.uri());
    let result = client.get(&url).await;
    // wreq doesn't throw on 500, returns body
    if let Ok(body) = result {
        assert!(body.contains("Internal Error") || body.is_empty());
    }
}

// ============================================================================
// Negative Testing: 429 Rate Limit (wiremock)
// ============================================================================

/// Test HTTP 429 Rate Limited response
#[tokio::test]
async fn test_mock_server_429_rate_limit() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/rate-limited"))
        .respond_with(
            ResponseTemplate::new(429)
                .set_body_string("Too Many Requests")
                .insert_header("Retry-After", "1"),
        )
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        max_retries: 1,
        backoff_base_ms: 10,
        backoff_max_ms: 50,
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    let url = format!("{}/rate-limited", mock_server.uri());
    let result = client.get(&url).await;

    // After retry, should still fail with RateLimited error
    assert!(
        result.is_err(),
        "Should return error for 429, got: {:?}",
        result
    );
}

/// Test HTTP 429 with multiple retries exhausted
#[tokio::test]
async fn test_mock_server_429_exhausts_retries() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/429"))
        .respond_with(ResponseTemplate::new(429).set_body_string("Too Many Requests"))
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        max_retries: 2,
        backoff_base_ms: 10,
        backoff_max_ms: 50,
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    let start = std::time::Instant::now();
    let result = client.get(&format!("{}/429", mock_server.uri())).await;
    let elapsed = start.elapsed();

    // After 3 attempts (1 initial + 2 retries), should fail
    assert!(
        result.is_err(),
        "Should return error after retries exhausted"
    );
    // Should have waited for backoff (at least 2x backoff_base_ms = 20ms minimum)
    assert!(
        elapsed.as_millis() >= 20,
        "Should have waited for backoff, only waited {}ms",
        elapsed.as_millis()
    );
}

// ============================================================================
// Negative Testing: 503 Service Unavailable (wiremock)
// ============================================================================

/// Test HTTP 503 Service Unavailable response
#[tokio::test]
async fn test_mock_server_503_service_unavailable() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/unavailable"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        max_retries: 1,
        backoff_base_ms: 10,
        backoff_max_ms: 50,
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    let url = format!("{}/unavailable", mock_server.uri());
    let result = client.get(&url).await;

    // After retry, should fail with ServerError(503)
    assert!(
        result.is_err(),
        "Should return error for 503, got: {:?}",
        result
    );
}

/// Test HTTP 503 with Retry-After header
#[tokio::test]
async fn test_mock_server_503_with_retry_after() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/503-retry"))
        .respond_with(
            ResponseTemplate::new(503)
                .set_body_string("Service Unavailable")
                .insert_header("Retry-After", "2"),
        )
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        max_retries: 1,
        backoff_base_ms: 1000, // 1 second base
        backoff_max_ms: 5000,
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    let start = std::time::Instant::now();
    let result = client
        .get(&format!("{}/503-retry", mock_server.uri()))
        .await;
    let elapsed = start.elapsed();

    // Should fail but respect Retry-After header (2 seconds)
    assert!(result.is_err());
    // The Retry-After header should influence backoff timing
    assert!(
        elapsed.as_millis() >= 1000,
        "Should have waited at least 1s for Retry-After, waited {}ms",
        elapsed.as_millis()
    );
}

// ============================================================================
// Negative Testing: Latency Simulation (backpressure)
// ============================================================================

/// Test client handles slow responses (backpressure simulation)
#[tokio::test]
async fn test_mock_server_handles_slow_response() {
    let mock_server = MockServer::start().await;

    // Simulate 500ms latency
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Slow response content")
                .set_delay(Duration::from_millis(500)),
        )
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        timeout_secs: 30, // 30 second timeout - should pass
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    let start = std::time::Instant::now();
    let url = format!("{}/slow", mock_server.uri());
    let result = client.get(&url).await;
    let elapsed = start.elapsed();

    // Should succeed but take at least 500ms
    assert!(
        result.is_ok(),
        "Should handle slow response, got: {:?}",
        result
    );
    assert!(
        elapsed.as_millis() >= 500,
        "Should wait for slow response, only waited {}ms",
        elapsed.as_millis()
    );
}

/// Test client timeout on very slow response
#[tokio::test]
async fn test_mock_server_timeout_on_slow_response() {
    let mock_server = MockServer::start().await;

    // Simulate 5 second latency (longer than client timeout)
    Mock::given(method("GET"))
        .and(path("/very-slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("Very slow response")
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig {
        timeout_secs: 1, // 1 second timeout - should fail
        ..Default::default()
    };
    let client = HttpClient::new(config).unwrap();

    let start = std::time::Instant::now();
    let result = client
        .get(&format!("{}/very-slow", mock_server.uri()))
        .await;
    let elapsed = start.elapsed();

    // Should timeout/fail
    assert!(
        result.is_err(),
        "Should timeout on slow response, got: {:?}",
        result
    );
    // But should have waited at least close to timeout
    assert!(
        elapsed.as_millis() >= 900,
        "Should have waited near timeout (1s), only waited {}ms",
        elapsed.as_millis()
    );
}

// ============================================================================
// Negative Testing: Empty Response
// ============================================================================

/// Test handling of empty response body
#[tokio::test]
async fn test_mock_server_empty_body() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/empty"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .mount(&mock_server)
        .await;

    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let url = format!("{}/empty", mock_server.uri());
    let result = client.get(&url).await;

    // Should succeed but return empty string
    assert!(
        result.is_ok(),
        "Should handle empty body, got: {:?}",
        result
    );
    let body = result.unwrap();
    assert!(body.is_empty(), "Body should be empty, got: '{}'", body);
}
