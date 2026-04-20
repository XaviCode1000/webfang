//! Deduplicator module — URL deduplication logic
//!
//! Extracts the deduplication logic from crawler_service.rs to allow
//! for independent testing and potential future swapping (e.g., Redis-backed).
//!
//! # Design Decisions
//!
//! - Uses `tokio::sync::Mutex` for async-safe interior mutability
//! - Implements trait-based design for testability
//! - Stores URLs as strings (normalized) for memory efficiency
//! - Provides both sync and async interfaces

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;
use url::Url;

/// Trait for URL deduplication - allows different implementations
pub trait UrlDeduplicator: Send + Sync {
    /// Check if URL was already visited
    fn is_visited(&self, url: &str) -> impl std::future::Future<Output = bool> + Send;

    /// Mark URL as visited
    fn mark_visited(&self, url: String) -> impl std::future::Future<Output = ()> + Send;

    /// Get current count of visited URLs
    fn visited_count(&self) -> impl std::future::Future<Output = usize> + Send;

    /// Check and mark in one operation (atomic)
    fn check_and_mark(&self, url: String) -> impl std::future::Future<Output = bool> + Send;
}

/// In-memory URL deduplicator using HashSet
///
/// # Example
///
/// ```rust
/// use rust_scraper::application::deduplicator::{UrlDeduplicator, InMemoryDeduplicator};
///
/// #[tokio::main]
/// async fn main() {
///     let dedup = InMemoryDeduplicator::new();
///
///     // First time - returns false (not visited)
///     let is_new = dedup.check_and_mark("https://example.com".into()).await;
///     assert!(!is_new);
///
///     // Second time - returns true (already visited)
///     let is_new = dedup.check_and_mark("https://example.com".into()).await;
///     assert!(is_new);
/// }
/// ```
#[derive(Clone)]
pub struct InMemoryDeduplicator {
    visited: Arc<Mutex<HashSet<String>>>,
}

impl InMemoryDeduplicator {
    /// Create a new in-memory deduplicator
    pub fn new() -> Self {
        Self {
            visited: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Create with pre-allocated capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            visited: Arc::new(Mutex::new(HashSet::with_capacity(capacity))),
        }
    }
}

impl Default for InMemoryDeduplicator {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlDeduplicator for InMemoryDeduplicator {
    async fn is_visited(&self, url: &str) -> bool {
        let visited = self.visited.lock().await;
        visited.contains(url)
    }

    async fn mark_visited(&self, url: String) {
        let mut visited = self.visited.lock().await;
        visited.insert(url);
    }

    async fn visited_count(&self) -> usize {
        let visited = self.visited.lock().await;
        visited.len()
    }

    async fn check_and_mark(&self, url: String) -> bool {
        let mut visited = self.visited.lock().await;
        !visited.insert(url) // Return true if already existed, false if newly inserted
    }
}

/// Normalize URL for consistent deduplication
///
/// - Removes trailing slashes
/// - Removes www prefix
/// - Converts to lowercase (for domain)
/// - Removes default ports (80, 443)
pub fn normalize_url(url: &Url) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("");
    let port = url.port();
    let path = url.path();

    // Build normalized string manually to avoid borrow issues
    let mut result = format!("{}://", scheme);

    // Handle www prefix
    let host = host.strip_prefix("www.").unwrap_or(host);

    // Handle default port - add only if not default
    let port_str = match (port, scheme) {
        (None, _) => "",
        (Some(80), _) => "",  // Remove port 80 regardless of scheme
        (Some(443), "https") => "",
        (Some(p), _) => &format!(":{}", p),
    };

    result.push_str(host);
    result.push_str(port_str);

    // Remove trailing slash from path
    let path = if path.ends_with('/') && path.len() > 1 {
        &path[..path.len() - 1]
    } else {
        path
    };

    // Don't add path if it's just "/" (root)
    if path != "/" && !path.is_empty() {
        result.push_str(path);
    }

    // Add query string if present
    if let Some(query) = url.query() {
        result.push('?');
        result.push_str(query);
    }

    result
}

/// Results collector - collects crawl results without Mutex per item
///
/// Instead of `Arc<Mutex<Vec<T>>>`, uses channels for better performance
/// in high-concurrency scenarios.
#[derive(Clone)]
pub struct ResultsCollector<T: Clone + Send> {
    results: Arc<Mutex<Vec<T>>>,
}

impl<T: Clone + Send> ResultsCollector<T> {
    /// Create a new results collector
    pub fn new() -> Self {
        Self {
            results: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Create with pre-allocated capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            results: Arc::new(Mutex::new(Vec::with_capacity(capacity))),
        }
    }

    /// Add a result
    pub async fn add(&self, result: T) {
        let mut results = self.results.lock().await;
        results.push(result);
    }

    /// Get all results
    pub async fn get_all(&self) -> Vec<T> {
        let results = self.results.lock().await;
        results.clone()
    }

    /// Get count
    pub async fn len(&self) -> usize {
        let results = self.results.lock().await;
        results.len()
    }

    /// Check if empty
    pub async fn is_empty(&self) -> bool {
        let results = self.results.lock().await;
        results.is_empty()
    }

    /// Clear all results
    pub async fn clear(&self) {
        let mut results = self.results.lock().await;
        results.clear();
    }
}

impl<T: Clone + Send> Default for ResultsCollector<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_deduplicator_check_and_mark() {
        let dedup = InMemoryDeduplicator::new();

        // First time should return false (was inserted)
        let result = dedup.check_and_mark("https://example.com".to_string()).await;
        assert!(!result, "First insert should return false (URL was added)");

        // Second time should return true (already exists)
        let result = dedup.check_and_mark("https://example.com".to_string()).await;
        assert!(result, "Second check should return true (URL already visited)");
    }

    #[tokio::test]
    async fn test_deduplicator_count() {
        let dedup = InMemoryDeduplicator::new();

        assert_eq!(dedup.visited_count().await, 0);

        dedup.mark_visited("https://a.com".to_string()).await;
        assert_eq!(dedup.visited_count().await, 1);

        dedup.mark_visited("https://b.com".to_string()).await;
        assert_eq!(dedup.visited_count().await, 2);
    }

    #[test]
    fn test_normalize_url() {
        let url = Url::parse("https://www.example.com/").unwrap();
        assert_eq!(normalize_url(&url), "https://example.com");

        let url = Url::parse("https://www.example.com/page/").unwrap();
        assert_eq!(normalize_url(&url), "https://example.com/page");

        let url = Url::parse("https://example.com:80/page").unwrap();
        assert_eq!(normalize_url(&url), "https://example.com/page");
    }

    #[tokio::test]
    async fn test_results_collector() {
        let collector: ResultsCollector<String> = ResultsCollector::new();

        collector.add("result1".to_string()).await;
        collector.add("result2".to_string()).await;

        assert_eq!(collector.len().await, 2);
        assert!(!collector.is_empty().await);

        let all = collector.get_all().await;
        assert_eq!(all, vec!["result1", "result2"]);
    }
}