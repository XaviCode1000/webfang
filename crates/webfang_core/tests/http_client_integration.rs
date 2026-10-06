//! Integration tests for HttpClient
//!
//! All tests use wiremock for deterministic, network-free HTTP responses.
//! Run with: cargo nextest run --test http_client_integration

use std::time::Duration;
use webfang_core::application::http_client::{HttpClient, HttpClientConfig, HttpError};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

// Shared arrange helpers (T7 slice-2): client builders with retry/backoff
// presets plus mock-setup + get helpers. This mirrors the unit-test helpers
// in `application::http_client::client` — integration tests cannot import
// `#[cfg(test)]` items, so the small shape lives here instead.

/// Fast retry preset: short backoff keeps the retry path exercised but the
/// test fast.
fn retry_config(max_retries: u32) -> HttpClientConfig {
    HttpClientConfig {
        max_retries,
        backoff_base_ms: 10,
        backoff_max_ms: 50,
        ..Default::default()
    }
}

/// Client with default config for status/body tests that never retry.
fn default_client() -> HttpClient {
    HttpClient::new(HttpClientConfig::default()).expect("client builds")
}

/// Client with the fast retry preset.
fn retry_client(max_retries: u32) -> HttpClient {
    HttpClient::new(retry_config(max_retries)).expect("client builds")
}

/// Client with a custom request timeout in seconds.
fn timeout_client(timeout_secs: u64) -> HttpClient {
    HttpClient::new(HttpClientConfig {
        timeout_secs,
        ..Default::default()
    })
    .expect("client builds")
}

/// Mount a `GET {route}` mock answering `status` with `body`.
async fn mount_body(server: &MockServer, route: &str, status: u16, body: &str) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

/// Mount a `GET {route}` mock answering `status` with `body` after `delay`.
async fn mount_delayed(server: &MockServer, route: &str, status: u16, body: &str, delay: Duration) {
    Mock::given(method("GET"))
        .and(path(route))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_string(body)
                .set_delay(delay),
        )
        .mount(server)
        .await;
}

/// GET `route` against `server` through `client`.
async fn get_route(
    client: &HttpClient,
    server: &MockServer,
    route: &str,
) -> Result<String, HttpError> {
    client.get(&format!("{}{route}", server.uri())).await
}

/// Test HTTP 200 OK with mock server
#[tokio::test]
async fn test_mock_server_200() {
    let mock_server = MockServer::start().await;
    mount_body(&mock_server, "/", 200, "<html>OK</html>").await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/").await;
    assert!(result.is_ok(), "Should succeed: {result:?}");
}

/// Test HTTP 404 with mock server: the client maps 4xx responses to a `ClientError`.
#[tokio::test]
async fn test_mock_server_404() {
    let mock_server = MockServer::start().await;
    mount_body(&mock_server, "/missing", 404, "Not Found").await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/missing").await;
    // HttpClient maps 404 to ClientError(404); 4xx responses are not retried.
    assert!(
        matches!(result, Err(HttpError::ClientError(404))),
        "Expected ClientError(404), got: {result:?}"
    );
}

/// Test HTTP 500 with mock server: the client retries 5xx, then surfaces `ServerError`.
#[tokio::test]
async fn test_mock_server_500() {
    let mock_server = MockServer::start().await;
    mount_body(&mock_server, "/error", 500, "Internal Error").await;

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/error").await;
    // Once retries are exhausted, 500 surfaces as ServerError(500).
    assert!(
        matches!(result, Err(HttpError::ServerError(500))),
        "Expected ServerError(500), got: {result:?}"
    );
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

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/rate-limited").await;

    // After retry, should still fail with RateLimited error
    assert!(
        result.is_err(),
        "Should return error for 429, got: {result:?}"
    );
}

/// Test HTTP 429 with multiple retries exhausted
#[tokio::test]
async fn test_mock_server_429_exhausts_retries() {
    let mock_server = MockServer::start().await;
    mount_body(&mock_server, "/429", 429, "Too Many Requests").await;

    let client = retry_client(2);

    let start = std::time::Instant::now();
    let result = get_route(&client, &mock_server, "/429").await;
    let elapsed = start.elapsed();

    // After 3 attempts (1 initial + 2 retries), should fail
    assert!(
        result.is_err(),
        "Should return error after retries exhausted"
    );
    // 429 backoff ignores backoff_base_ms: with no Retry-After header the client
    // defaults to a 1s per-retry delay, so 2 retries wait ~2s total (well over 20ms).
    assert!(
        elapsed.as_millis() >= 20,
        "Should have waited through the retry delays, only waited {}ms",
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
    mount_body(&mock_server, "/unavailable", 503, "Service Unavailable").await;

    let client = retry_client(1);

    let result = get_route(&client, &mock_server, "/unavailable").await;

    // After retry, should fail with ServerError(503)
    assert!(
        result.is_err(),
        "Should return error for 503, got: {result:?}"
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

    // Should fail with a server error after retries.
    assert!(result.is_err());
    // 5xx retries ignore the Retry-After header and use exponential backoff: the
    // ~1s wait comes from backoff_base_ms (1000ms * 2^0), not from Retry-After.
    assert!(
        elapsed.as_millis() >= 1000,
        "Should have waited at least 1s of backoff, waited {}ms",
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
    mount_delayed(
        &mock_server,
        "/slow",
        200,
        "Slow response content",
        Duration::from_millis(500),
    )
    .await;

    let client = timeout_client(30); // 30 second timeout - should pass

    let start = std::time::Instant::now();
    let result = get_route(&client, &mock_server, "/slow").await;
    let elapsed = start.elapsed();

    // Should succeed but take at least 500ms
    assert!(
        result.is_ok(),
        "Should handle slow response, got: {result:?}"
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
    mount_delayed(
        &mock_server,
        "/very-slow",
        200,
        "Very slow response",
        Duration::from_secs(5),
    )
    .await;

    let client = timeout_client(1); // 1 second timeout - should fail

    let start = std::time::Instant::now();
    let result = get_route(&client, &mock_server, "/very-slow").await;
    let elapsed = start.elapsed();

    // Should timeout/fail
    assert!(
        result.is_err(),
        "Should timeout on slow response, got: {result:?}"
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
    mount_body(&mock_server, "/empty", 200, "").await;

    let client = default_client();

    let result = get_route(&client, &mock_server, "/empty").await;

    // Should succeed but return empty string
    assert!(result.is_ok(), "Should handle empty body, got: {result:?}");
    let body = result.unwrap();
    assert!(body.is_empty(), "Body should be empty, got: '{body}'");
}

/// Test handling of oversized response body: returns BodyTooLarge error
#[tokio::test]
async fn test_mock_server_oversized_body() {
    let mock_server = MockServer::start().await;

    // Body size: 2 KiB (2048 bytes)
    let oversized_body = vec![b'x'; 2048];
    Mock::given(method("GET"))
        .and(path("/oversized"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(std::str::from_utf8(&oversized_body).unwrap())
                .insert_header("content-type", "text/plain"),
        )
        .mount(&mock_server)
        .await;

    // Configure client with a small limit: 1 KiB (1024 bytes)
    let config = HttpClientConfig {
        max_page_bytes: 1024, // 1 KiB
        ..HttpClientConfig::default()
    };
    let client = HttpClient::new(config).unwrap();

    let url = format!("{}/oversized", mock_server.uri());
    let result = client.get(&url).await;

    // Should fail with BodyTooLarge error
    assert!(matches!(
        result,
        Err(HttpError::BodyTooLarge { limit: 1024 })
    ));
}

/// Test normal body still works under the limit (regression test)
#[tokio::test]
async fn test_mock_server_normal_body_under_limit() {
    let mock_server = MockServer::start().await;

    let normal_body = b"Hello, world!";
    Mock::given(method("GET"))
        .and(path("/normal"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(std::str::from_utf8(normal_body).unwrap())
                .insert_header("content-type", "text/plain"),
        )
        .mount(&mock_server)
        .await;

    // Configure client with a limit larger than the body: 2 KiB
    let config = HttpClientConfig {
        max_page_bytes: 2 * 1024, // 2 KiB
        ..HttpClientConfig::default()
    };
    let client = HttpClient::new(config).unwrap();

    let url = format!("{}/normal", mock_server.uri());
    let result = client.get(&url).await;

    // Should succeed and return the body
    assert!(result.is_ok(), "Should succeed: {result:?}");
    let body = result.unwrap();
    assert_eq!(body, std::str::from_utf8(normal_body).unwrap());
}
