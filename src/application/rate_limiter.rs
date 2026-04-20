//! Rate Limiter module — Token Bucket implementation using governor
//!
//! Extracts the rate limiting logic from crawler_service.rs to allow
//! for independent testing and potential future swapping (e.g., Redis-backed).
//!
//! # Design Decisions
//!
//! - Uses `governor` crate with Token Bucket algorithm
//! - Thread-safe via Arc (shares across async tasks)
//! - Configurable delay and burst parameters
//! - No Mutex needed - governor handles internal synchronization

use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use governor::{
    clock::QuantaClock,
    state::{InMemoryState, NotKeyed},
    Quota, RateLimiter,
};

use crate::error::ScraperError;

/// Type alias for the rate limiter - allows swapping implementations
pub type CrawlRateLimiter = RateLimiter<NotKeyed, InMemoryState, QuantaClock>;

/// Rate limiter configuration
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Delay between requests in milliseconds
    pub delay_ms: u64,
    /// Maximum concurrent requests (burst)
    pub concurrency: u32,
}

impl RateLimiterConfig {
    /// Create new configuration
    pub fn new(delay_ms: u64, concurrency: u32) -> Self {
        Self {
            delay_ms,
            concurrency,
        }
    }
}

/// Shared rate limiter for crawl operations
#[derive(Clone)]
pub struct SharedRateLimiter(Arc<CrawlRateLimiter>);

impl SharedRateLimiter {
    /// Create a new shared rate limiter from config
    pub fn new(config: &RateLimiterConfig) -> Result<Self, ScraperError> {
        let quota = Quota::with_period(Duration::from_millis(config.delay_ms))
            .ok_or_else(|| ScraperError::Config("Invalid period".into()))?;

        let quota = quota.allow_burst(
            NonZeroU32::new(config.concurrency)
                .ok_or_else(|| ScraperError::Config("Concurrency must be > 0".into()))?,
        );

        let limiter = RateLimiter::direct(quota);
        Ok(Self(Arc::new(limiter)))
    }

    /// Wait until a permit is available
    pub async fn until_ready(&self) {
        self.0.until_ready().await;
    }
}

impl From<RateLimiter<NotKeyed, InMemoryState, QuantaClock>> for SharedRateLimiter {
    fn from(limiter: RateLimiter<NotKeyed, InMemoryState, QuantaClock>) -> Self {
        Self(Arc::new(limiter))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_config_default() {
        let config = RateLimiterConfig::new(100, 5);
        assert_eq!(config.delay_ms, 100);
        assert_eq!(config.concurrency, 5);
    }

    #[tokio::test]
    async fn test_rate_limiter_creation() {
        let config = RateLimiterConfig::new(50, 2);
        let limiter = SharedRateLimiter::new(&config);
        assert!(limiter.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limiter_until_ready() {
        let config = RateLimiterConfig::new(10, 1);
        let limiter = SharedRateLimiter::new(&config).unwrap();
        
        // Wait for permit availability
        limiter.until_ready().await;
        
        // If we got here, the limiter worked
    }

    #[test]
    fn test_rate_limiter_clone_ischeap() {
        let config = RateLimiterConfig::new(100, 5);
        let limiter = SharedRateLimiter::new(&config).unwrap();
        
        // Cloning should be cheap (just Arc increment)
        let _clone = limiter.clone();
    }
}