//! Per-domain session pool for rate limiting.
//!
//! Tracks request timing per domain to enforce cooldown periods between
//! requests to the same host, preventing 429/403 responses.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, warn};

use crate::domain::CrawlError;

/// Per-domain session health tracker.
///
/// Uses a read-heavy `RwLock<HashMap>` — the lock is held only during
/// `acquire()` / `report_*()` calls (microseconds), never across `.await`.
#[derive(Debug, Clone)]
pub struct DomainSessionPool {
    inner: Arc<RwLock<HashMap<String, DomainSession>>>,
    /// Minimum time between requests to the same domain.
    cooldown: Duration,
    /// Max consecutive failures before marking a domain unhealthy.
    max_failures: u32,
}

#[derive(Debug, Clone)]
struct DomainSession {
    last_request: Option<Instant>,
    consecutive_failures: u32,
    total_requests: u64,
}

impl DomainSession {
    fn new() -> Self {
        Self {
            last_request: None,
            consecutive_failures: 0,
            total_requests: 0,
        }
    }
}

impl DomainSessionPool {
    /// Create a new session pool with the given cooldown between requests.
    pub fn new(cooldown: Duration, max_failures: u32) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            cooldown,
            max_failures,
        }
    }

    /// Create a pool with default settings (2s cooldown, 5 max failures).
    #[must_use]
    pub fn default_pool() -> Self {
        Self::new(Duration::from_secs(2), 5)
    }

    /// Check if a domain is healthy and acquire a permit.
    ///
    /// Returns `Ok(true)` if the request can proceed.
    /// Returns `Ok(false)` if the domain is rate-limited (cooldown not elapsed).
    /// Returns `Err` if the domain is unhealthy (too many consecutive failures).
    pub async fn acquire(&self, domain: &str) -> Result<bool, CrawlError> {
        let mut sessions = self.inner.write().await;
        let session = sessions
            .entry(domain.to_string())
            .or_insert_with(DomainSession::new);

        // Check if domain is unhealthy
        if session.consecutive_failures >= self.max_failures {
            warn!(
                domain = domain,
                failures = session.consecutive_failures,
                "Domain marked unhealthy — skipping"
            );
            return Err(CrawlError::SessionPool(format!(
                "domain {domain} marked unhealthy after {} failures",
                session.consecutive_failures
            )));
        }

        // Check cooldown
        if let Some(last) = session.last_request {
            let elapsed = last.elapsed();
            if elapsed < self.cooldown {
                debug!(
                    domain = domain,
                    remaining_ms = (self.cooldown - elapsed).as_millis() as u64,
                    "Domain on cooldown — deferring"
                );
                return Ok(false);
            }
        }

        session.last_request = Some(Instant::now());
        session.total_requests += 1;
        Ok(true)
    }

    /// Report a successful request to a domain.
    pub async fn report_success(&self, domain: &str) {
        let mut sessions = self.inner.write().await;
        if let Some(session) = sessions.get_mut(domain) {
            session.consecutive_failures = 0;
        }
    }

    /// Report a failed request to a domain.
    pub async fn report_failure(&self, domain: &str) {
        let mut sessions = self.inner.write().await;
        let session = sessions
            .entry(domain.to_string())
            .or_insert_with(DomainSession::new);
        session.consecutive_failures += 1;
        session.last_request = Some(Instant::now());
    }

    /// Check if a domain is healthy.
    #[must_use]
    pub async fn is_healthy(&self, domain: &str) -> bool {
        let sessions = self.inner.read().await;
        sessions
            .get(domain)
            .map(|s| s.consecutive_failures < self.max_failures)
            .unwrap_or(true)
    }

    /// Get stats for a domain.
    #[must_use]
    pub async fn stats(&self, domain: &str) -> Option<(u64, u32)> {
        let sessions = self.inner.read().await;
        sessions
            .get(domain)
            .map(|s| (s.total_requests, s.consecutive_failures))
    }

    /// Get the number of tracked domains.
    #[must_use]
    pub async fn domain_count(&self) -> usize {
        let sessions = self.inner.read().await;
        sessions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_acquire_first_request_always_allowed() {
        let pool = DomainSessionPool::new(Duration::from_secs(1), 5);
        assert!(pool.acquire("example.com").await.unwrap());
    }

    #[tokio::test]
    async fn test_acquire_cooldown_defers() {
        let pool = DomainSessionPool::new(Duration::from_secs(60), 5);
        assert!(pool.acquire("example.com").await.unwrap());
        // Second request within cooldown
        assert!(!pool.acquire("example.com").await.unwrap());
    }

    #[tokio::test]
    async fn test_report_failure_increments() {
        let pool = DomainSessionPool::new(Duration::from_secs(0), 2);
        pool.report_failure("example.com").await;
        pool.report_failure("example.com").await;
        // Now unhealthy
        assert!(pool.acquire("example.com").await.is_err());
    }

    #[tokio::test]
    async fn test_report_success_resets_failures() {
        let pool = DomainSessionPool::new(Duration::from_secs(0), 2);
        pool.report_failure("example.com").await;
        pool.report_success("example.com").await;
        assert!(pool.acquire("example.com").await.unwrap());
    }

    #[tokio::test]
    async fn test_different_domains_independent() {
        let pool = DomainSessionPool::new(Duration::from_secs(60), 5);
        assert!(pool.acquire("a.com").await.unwrap());
        // Different domain not affected by cooldown
        assert!(pool.acquire("b.com").await.unwrap());
    }

    #[tokio::test]
    async fn test_domain_count() {
        let pool = DomainSessionPool::new(Duration::from_secs(1), 5);
        pool.acquire("a.com").await.unwrap();
        pool.acquire("b.com").await.unwrap();
        assert_eq!(pool.domain_count().await, 2);
    }

    #[tokio::test]
    async fn test_stats() {
        let pool = DomainSessionPool::new(Duration::from_secs(0), 5);
        pool.acquire("example.com").await.unwrap();
        pool.report_failure("example.com").await;

        let (requests, failures) = pool.stats("example.com").await.unwrap();
        assert_eq!(requests, 1);
        assert_eq!(failures, 1);
    }
}
