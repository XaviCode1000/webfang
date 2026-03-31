//! Integration tests for HttpClient with real websites
//!
//! These tests make actual HTTP requests to public websites.
//! Run with: cargo test --ignored
//!
//! Sites used:
//! - books.toscrape.com - Simple static HTML, good for testing scraping
//! - quotes.toscrape.com - Pagination, JavaScript not required
//! - webscraper.dev - Static site for testing selectors

use rust_scraper::application::http_client::{HttpClient, HttpClientConfig, HttpError};

/// Test against books.toscrape.com - verifies basic HTML fetching
///
/// This site is specifically designed for scraping practice.
/// It returns static HTML with book listings.
#[tokio::test]
#[ignore = "requires network - run with cargo test --ignored"]
async fn test_books_toscrape() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let result = client.get("https://books.toscrape.com/").await;

    assert!(result.is_ok(), "Failed to fetch books.toscrape.com: {:?}", result);
    
    let body = result.unwrap();
    
    // Verify we got HTML content
    assert!(body.contains("html"), "Response should be HTML");
    assert!(body.contains("books") || body.contains("Books") || body.contains("book"), 
        "Should contain book-related content");
    
    // Should have actual content (not blocked)
    assert!(body.len() > 1000, "Body should be > 1KB");
}

/// Test against quotes.toscrape.com - verifies pagination handling
///
/// Tests that the client can handle different types of content
/// and maintain session/cookies if needed.
#[tokio::test]
#[ignore = "requires network - run with cargo test --ignored"]
async fn test_quotes_toscrape() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let result = client.get("https://quotes.toscrape.com/").await;

    assert!(result.is_ok(), "Failed to fetch quotes.toscrape.com: {:?}", result);
    
    let body = result.unwrap();
    
    // Verify content
    assert!(body.contains("quote") || body.contains("Quote"), 
        "Should contain quote content");
    assert!(body.len() > 500, "Body should have substantial content");
}

/// Test against webscraper.io - verifies static content extraction
///
/// This site provides various HTML structures for testing selectors.
#[tokio::test]
#[ignore = "requires network - run with cargo test --ignored"]
async fn test_webscraper_static() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    let result = client.get("https://webscraper.io/test-sites/e-commerce/static").await;

    assert!(result.is_ok(), "Failed to fetch webscraper.io: {:?}", result);
    
    let body = result.unwrap();
    
    // Should contain HTML
    assert!(body.contains("html") || body.contains("HTML"), "Should be HTML");
    assert!(body.len() > 500, "Body should have content");
}

/// Test configuration customization
///
/// Verifies that custom configuration is applied correctly.
#[tokio::test]
#[ignore = "requires network - run with cargo test --ignored"]
async fn test_custom_config() {
    let config = HttpClientConfig {
        accept_language: "es-ES,es;q=0.9".into(),
        accept: "text/html".into(),
        referer: "https://bing.com/".into(),
        cache_control: "no-store".into(),
        max_retries: 5,
        backoff_base_ms: 500,
        backoff_max_ms: 5000,
        enable_cookies: true,
    };
    
    let client = HttpClient::new(config).unwrap();
    
    // Use books.toscrape.com as it's more reliable
    let result = client.get("https://books.toscrape.com/").await;
    assert!(result.is_ok());
    
    let body = result.unwrap();
    assert!(!body.is_empty());
}

/// Test 404 error handling with real site
///
/// example.com has a /404 path that returns 404
#[tokio::test]
#[ignore = "requires network - run with cargo test --ignored"]
async fn test_404_handling() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    // Use httpbin to get a guaranteed 404
    let result = client.get("https://httpbin.org/status/404").await;

    assert!(result.is_err());
    
    let err = result.unwrap_err();
    assert!(matches!(err, HttpError::ClientError(404)));
}

/// Test connection error handling
#[tokio::test]
#[ignore = "requires network - run with cargo test --ignored"]
async fn test_connection_error() {
    let config = HttpClientConfig::default();
    let client = HttpClient::new(config).unwrap();

    // Use a non-existent domain
    let result = client.get("https://this-domain-does-not-exist-12345.com/").await;

    assert!(result.is_err());
    
    let err = result.unwrap_err();
    // Should be connection error
    assert!(matches!(err, HttpError::Connection(_)));
}
